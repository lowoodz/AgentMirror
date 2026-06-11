//! Sanitize upstream SSE chunks before forwarding to OpenAI-compatible clients.

use serde_json::{Map, Value};

/// For OpenAI-compatible clients, fold `reasoning_content` into `content` and drop the field.
/// OpenClaw `openai-completions` cannot parse bare `reasoning_content` deltas.
pub fn sanitize_openai_client_sse_chunk(chunk: &mut Value) -> bool {
    let Some(choices) = chunk.get_mut("choices").and_then(|c| c.as_array_mut()) else {
        return true;
    };
    let mut keep = false;
    for choice in choices {
        if let Some(delta) = choice.get_mut("delta").and_then(|d| d.as_object_mut()) {
            fold_reasoning_into_content(delta);
            if delta_has_payload(delta) {
                keep = true;
            }
        }
        if choice
            .get("finish_reason")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty())
        {
            keep = true;
        }
    }
    keep
}

fn fold_reasoning_into_content(obj: &mut Map<String, Value>) -> bool {
    let Some(reasoning) = obj.remove("reasoning_content") else {
        return false;
    };
    let Some(r_text) = reasoning.as_str() else {
        return true;
    };
    if r_text.is_empty() {
        return true;
    }
    match obj.get_mut("content") {
        Some(Value::String(c)) => c.push_str(r_text),
        Some(Value::Null) | None => {
            obj.insert("content".into(), Value::String(r_text.to_string()));
        }
        _ => {}
    }
    true
}

fn delta_has_payload(delta: &Map<String, Value>) -> bool {
    if delta.get("tool_calls").is_some() {
        return true;
    }
    if delta
        .get("content")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty())
    {
        return true;
    }
    if delta.len() == 1 && delta.contains_key("role") {
        return true;
    }
    delta
        .keys()
        .any(|k| k.as_str() != "content" && k.as_str() != "role")
}

/// Strip DeepSeek `reasoning_content` from buffered OpenAI chat completion JSON.
pub fn sanitize_openai_client_json(body: &mut Value) -> bool {
    let Some(choices) = body.get_mut("choices").and_then(|c| c.as_array_mut()) else {
        return false;
    };
    let mut modified = false;
    for choice in choices {
        if let Some(message) = choice.get_mut("message").and_then(|m| m.as_object_mut()) {
            modified |= fold_reasoning_into_content(message);
        }
        if let Some(delta) = choice.get_mut("delta").and_then(|d| d.as_object_mut()) {
            modified |= fold_reasoning_into_content(delta);
        }
    }
    modified
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn folds_reasoning_only_delta_into_content() {
        let mut chunk = json!({
            "choices": [{
                "delta": {"content": null, "reasoning_content": "thinking", "role": "assistant"},
                "index": 0
            }]
        });
        assert!(sanitize_openai_client_sse_chunk(&mut chunk));
        assert_eq!(chunk["choices"][0]["delta"]["content"], "thinking");
        assert!(chunk["choices"][0]["delta"].get("reasoning_content").is_none());
    }

    #[test]
    fn keeps_role_only_stream_start() {
        let mut chunk = json!({
            "choices": [{
                "delta": {"role": "assistant"},
                "index": 0
            }]
        });
        assert!(sanitize_openai_client_sse_chunk(&mut chunk));
    }

    #[test]
    fn keeps_tool_call_delta_after_reasoning_strip() {
        let mut chunk = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{"index": 0, "function": {"name": "exec"}}],
                    "reasoning_content": "x"
                },
                "index": 0
            }]
        });
        assert!(sanitize_openai_client_sse_chunk(&mut chunk));
        assert!(chunk["choices"][0]["delta"].get("tool_calls").is_some());
    }

    #[test]
    fn strips_reasoning_from_buffered_message_json() {
        let mut body = json!({
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "content": "",
                    "reasoning_content": "think",
                    "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "exec", "arguments": "{}"}}]
                }
            }]
        });
        assert!(sanitize_openai_client_json(&mut body));
        assert!(body["choices"][0]["message"].get("reasoning_content").is_none());
        assert_eq!(body["choices"][0]["message"]["content"], "think");
    }
}
