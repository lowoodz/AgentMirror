use bytes::Bytes;
use smr_protocol::{extract_texts, inject_response_texts, parse_json_body, serialize_json_body};

use crate::ops::OperationSecurity;
use crate::sse_sanitize::sanitize_openai_client_sse_chunk;
use crate::sse_tool_ops::transform_buffered_sse_ops;

/// Scan SSE chunks: DLP (response-side file/content redaction) and operation security.
pub fn process_sse_response(
    body: &Bytes,
    session_id: &str,
    dlp: Option<&crate::dlp::DlpEngine>,
    ops: Option<&OperationSecurity>,
) -> anyhow::Result<(Bytes, u32, u32, u32)> {
    let mut text = body.to_vec();
    let mut blocks = 0u32;
    let mut observes = 0u32;
    let mut dlp_count = 0u32;
    let mut modified = false;

    if ops.is_some() {
        let body_str = String::from_utf8_lossy(&text);
        let (transformed, gate_blocks) = transform_buffered_sse_ops(&body_str, ops);
        if gate_blocks > 0 {
            modified = true;
            blocks += gate_blocks;
        }
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
        Ok((Bytes::from(new_body), blocks, observes, dlp_count))
    } else {
        Ok((body.clone(), blocks, observes, dlp_count))
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
}
