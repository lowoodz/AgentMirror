use bytes::Bytes;
use smr_protocol::{extract_texts, inject_response_texts, parse_json_body, serialize_json_body};

use crate::ops::OperationSecurity;
use crate::sse_sanitize::sanitize_openai_client_sse_chunk;
use crate::ops::OpsBlockKinds;
use crate::sse_tool_ops::transform_buffered_sse_ops;

/// Scan SSE chunks: DLP (response-side file/content redaction) and operation security.
pub fn process_sse_response(
    body: &Bytes,
    session_id: &str,
    dlp: Option<&crate::dlp::DlpEngine>,
    ops: Option<&OperationSecurity>,
) -> anyhow::Result<(Bytes, u32, u32, u32, OpsBlockKinds)> {
    let mut text = body.to_vec();
    let mut blocks = 0u32;
    let observes = 0u32;
    let mut dlp_count = 0u32;
    let mut modified = false;
    let mut block_kinds = OpsBlockKinds::default();

    if ops.is_some() {
        let body_str = String::from_utf8_lossy(&text);
        let (transformed, gate_blocks, kinds) = transform_buffered_sse_ops(&body_str, ops);
        if gate_blocks > 0 {
            modified = true;
            blocks += gate_blocks;
        }
        block_kinds.merge(kinds);
        if transformed != body_str {
            modified = true;
            text = transformed.into_bytes();
        }
    }

    let body_str = String::from_utf8_lossy(&text);
    let mut new_body = String::new();
    let mut saw_first_token = false;

    for line in body_str.lines() {
        if let Some(data) = line.strip_prefix("data: ") {
            if data.trim() == "[DONE]" {
                new_body.push_str(line);
                new_body.push('\n');
                continue;
            }
            if let Ok(mut json) = parse_json_body(data.as_bytes()) {
                if !sanitize_openai_client_sse_chunk(&mut json) {
                    modified = true;
                    continue;
                }
                if !saw_first_token && crate::router::sse_has_first_token(data.as_bytes()) {
                    saw_first_token = true;
                }

                if let Some(dlp) = dlp {
                    let extracted = extract_texts(&json)?;
                    let (replacements, count) =
                        dlp.process_response(session_id, &json, &extracted)?;
                    dlp_count += count as u32;
                    if !replacements.is_empty() {
                        inject_response_texts(&mut json, &replacements)?;
                        let patched = String::from_utf8(serialize_json_body(&json)?)?;
                        new_body.push_str("data: ");
                        new_body.push_str(&patched);
                        new_body.push('\n');
                        modified = true;
                        continue;
                    }
                }
            }
        }
        new_body.push_str(line);
        new_body.push('\n');
    }

    if modified {
        Ok((Bytes::from(new_body), blocks, observes, dlp_count, block_kinds))
    } else {
        Ok((body.clone(), blocks, observes, dlp_count, block_kinds))
    }
}

pub fn is_sse_content_type(headers: &http::HeaderMap) -> bool {
    headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("text/event-stream"))
        .unwrap_or(false)
}

pub fn request_wants_stream(body: &[u8]) -> bool {
    parse_json_body(body)
        .ok()
        .and_then(|j| j.get("stream").and_then(|s| s.as_bool()))
        .unwrap_or(false)
}

pub fn request_has_tools(json: &serde_json::Value) -> bool {
    json.get("tools")
        .or_else(|| json.get("functions"))
        .and_then(|t| t.as_array())
        .is_some_and(|a| !a.is_empty())
}

/// Upstream chat APIs reject `stream_options` unless `stream=true` (DeepSeek 400).
pub fn force_upstream_non_stream(json: &mut serde_json::Value) {
    json["stream"] = serde_json::Value::Bool(false);
    if let Some(obj) = json.as_object_mut() {
        obj.remove("stream_options");
    }
}

/// Ask OpenAI-compatible upstreams to include token usage in the final SSE chunk (AgentMirror).
pub fn ensure_openai_stream_usage(json: &mut serde_json::Value) {
    if json.get("stream").and_then(|s| s.as_bool()) != Some(true) {
        return;
    }
    if request_has_tools(json) {
        return;
    }
    match json.get_mut("stream_options") {
        Some(opts) if opts.is_object() => {
            if opts.get("include_usage").is_none() {
                opts["include_usage"] = serde_json::Value::Bool(true);
            }
        }
        Some(_) => {
            json["stream_options"] = serde_json::json!({ "include_usage": true });
        }
        None => {
            json["stream_options"] = serde_json::json!({ "include_usage": true });
        }
    }
}

/// Synthesize OpenAI SSE from a buffered chat completion (OpenClaw expects stream when stream:true).
pub fn openai_chat_completion_to_sse(completion: &serde_json::Value) -> anyhow::Result<Bytes> {
    use serde_json::json;

    let id = completion
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("smr-synth");
    let model = completion
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let created = completion.get("created").and_then(|v| v.as_i64()).unwrap_or(0);
    let choice0 = completion.get("choices").and_then(|c| c.get(0));
    let message = choice0.and_then(|c| c.get("message"));
    let finish = choice0
        .and_then(|c| c.get("finish_reason"))
        .and_then(|v| v.as_str())
        .unwrap_or("stop");

    let mut out = String::new();
    let base = || {
        json!({
            "id": id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
        })
    };

    let mut role = base();
    role["choices"] = json!([{"index": 0, "delta": {"role": "assistant"}, "finish_reason": null}]);
    append_sse_line(&mut out, &role);

    if let Some(msg) = message {
        if let Some(content) = msg.get("content").and_then(|v| v.as_str()) {
            if !content.is_empty() {
                let mut chunk = base();
                chunk["choices"] = json!([{
                    "index": 0,
                    "delta": {"content": content},
                    "finish_reason": null
                }]);
                append_sse_line(&mut out, &chunk);
            }
        }
        if let Some(tool_calls) = msg.get("tool_calls") {
            let mut chunk = base();
            chunk["choices"] = json!([{
                "index": 0,
                "delta": {"tool_calls": tool_calls},
                "finish_reason": null
            }]);
            append_sse_line(&mut out, &chunk);
        }
    }

    let mut fin = base();
    fin["choices"] = json!([{"index": 0, "delta": {}, "finish_reason": finish}]);
    append_sse_line(&mut out, &fin);
    if let Some(usage) = completion.get("usage") {
        if usage.is_object() && !usage.as_object().is_some_and(|o| o.is_empty()) {
            let mut usage_chunk = base();
            usage_chunk["choices"] = json!([]);
            usage_chunk["usage"] = usage.clone();
            append_sse_line(&mut out, &usage_chunk);
        }
    }
    out.push_str("data: [DONE]\n\n");
    Ok(Bytes::from(out))
}

fn append_sse_line(out: &mut String, value: &serde_json::Value) {
    out.push_str("data: ");
    out.push_str(&value.to_string());
    out.push_str("\n\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn force_upstream_non_stream_strips_stream_options() {
        let mut body = json!({
            "stream": true,
            "stream_options": {"include_usage": true},
            "tools": [{"type": "function"}]
        });
        force_upstream_non_stream(&mut body);
        assert_eq!(body.get("stream"), Some(&json!(false)));
        assert!(body.get("stream_options").is_none());
    }

    #[test]
    fn ensure_openai_stream_usage_adds_include_usage() {
        let mut body = json!({"stream": true, "model": "gpt"});
        ensure_openai_stream_usage(&mut body);
        assert_eq!(
            body["stream_options"]["include_usage"],
            json!(true)
        );
    }

    #[test]
    fn ensure_openai_stream_usage_skips_tools() {
        let mut body = json!({
            "stream": true,
            "tools": [{"type": "function", "function": {"name": "x", "parameters": {}}}]
        });
        ensure_openai_stream_usage(&mut body);
        assert!(body.get("stream_options").is_none());
    }

    #[test]
    fn openai_chat_completion_to_sse_includes_usage_chunk() {
        let completion = json!({
            "id": "c1",
            "model": "m",
            "created": 1,
            "choices": [{
                "message": {"role": "assistant", "content": "ok"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7}
        });
        let sse = openai_chat_completion_to_sse(&completion).unwrap();
        let text = String::from_utf8_lossy(&sse);
        assert!(text.contains(r#""usage""#));
        assert!(text.contains(r#""total_tokens":7"#));
        let usage = smr_insight::usage::extract_token_usage(&sse);
        assert_eq!(usage.total_tokens, 7);
    }
}
