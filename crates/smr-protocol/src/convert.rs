//! Convert OpenAI chat/completions requests to Anthropic /v1/messages format.

use serde_json::{json, Value};

use crate::protocol::ApiProtocol;

pub fn openai_to_anthropic(body: &Value) -> Value {
    let messages = body.get("messages").and_then(|m| m.as_array()).cloned().unwrap_or_default();
    let mut system_parts: Vec<String> = Vec::new();
    let mut out_messages: Vec<Value> = Vec::new();

    for msg in messages {
        let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");
        match role {
            "system" => {
                if let Some(s) = msg.get("content").and_then(|c| c.as_str()) {
                    system_parts.push(s.to_string());
                }
            }
            "tool" => {
                let content = msg.get("content").and_then(|c| c.as_str()).unwrap_or("");
                let tool_call_id = msg.get("tool_call_id").and_then(|c| c.as_str()).unwrap_or("tool");
                out_messages.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": tool_call_id,
                        "content": content
                    }]
                }));
            }
            "assistant" => {
                let mut blocks: Vec<Value> = Vec::new();
                if let Some(text) = msg.get("content").and_then(|c| c.as_str()) {
                    if !text.is_empty() {
                        blocks.push(json!({"type": "text", "text": text}));
                    }
                }
                if let Some(tool_calls) = msg.get("tool_calls").and_then(|t| t.as_array()) {
                    for tc in tool_calls {
                        let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("tool");
                        let name = tc.get("function").and_then(|f| f.get("name")).and_then(|n| n.as_str()).unwrap_or("");
                        let args = tc.get("function").and_then(|f| f.get("arguments")).and_then(|a| a.as_str()).unwrap_or("{}");
                        let input: Value = serde_json::from_str(args).unwrap_or(json!(args));
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": id,
                            "name": name,
                            "input": input
                        }));
                    }
                }
                if !blocks.is_empty() {
                    out_messages.push(json!({"role": "assistant", "content": blocks}));
                }
            }
            _ => {
                let content = msg.get("content").cloned().unwrap_or(json!(""));
                out_messages.push(json!({"role": role, "content": content}));
            }
        }
    }

    let max_tokens = body.get("max_tokens").and_then(|v| v.as_u64()).unwrap_or(4096);
    let mut out = json!({
        "model": body.get("model").cloned().unwrap_or(json!("")),
        "max_tokens": max_tokens,
        "messages": out_messages,
    });
    if !system_parts.is_empty() {
        out["system"] = json!(system_parts.join("\n"));
    }
    if let Some(temp) = body.get("temperature") {
        out["temperature"] = temp.clone();
    }
    if let Some(stream) = body.get("stream") {
        out["stream"] = stream.clone();
    }
    out
}

pub fn anthropic_to_openai(body: &Value) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if let Some(system) = body.get("system").and_then(|s| s.as_str()) {
        messages.push(json!({"role": "system", "content": system}));
    }
    if let Some(msgs) = body.get("messages").and_then(|m| m.as_array()) {
        for msg in msgs {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
                messages.push(json!({"role": role, "content": content}));
                continue;
            }
            if let Some(blocks) = msg.get("content").and_then(|c| c.as_array()) {
                if role == "assistant" {
                    let mut text = String::new();
                    let mut tool_calls = Vec::new();
                    for b in blocks {
                        match b.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                            "text" => text.push_str(b.get("text").and_then(|t| t.as_str()).unwrap_or("")),
                            "tool_use" => tool_calls.push(json!({
                                "id": b.get("id").cloned().unwrap_or(json!("tool")),
                                "type": "function",
                                "function": {
                                    "name": b.get("name").cloned().unwrap_or(json!("")),
                                    "arguments": b.get("input").cloned().unwrap_or(json!({})).to_string()
                                }
                            })),
                            _ => {}
                        }
                    }
                    let mut m = json!({"role": "assistant", "content": text});
                    if !tool_calls.is_empty() {
                        m["tool_calls"] = json!(tool_calls);
                    }
                    messages.push(m);
                } else {
                    for b in blocks {
                        if b.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                            messages.push(json!({
                                "role": "tool",
                                "tool_call_id": b.get("tool_use_id").cloned().unwrap_or(json!("tool")),
                                "content": b.get("content").cloned().unwrap_or(json!(""))
                            }));
                        }
                    }
                }
            }
        }
    }
    json!({
        "model": body.get("model").cloned().unwrap_or(json!("")),
        "messages": messages,
        "stream": body.get("stream").cloned().unwrap_or(json!(false))
    })
}

pub fn target_path(source_path: &str, to_anthropic: bool) -> String {
    if to_anthropic {
        if source_path.contains("chat/completions") {
            return source_path.replace("chat/completions", "messages");
        }
        return "/v1/messages".to_string();
    }
    if source_path.contains("messages") {
        return source_path.replace("messages", "chat/completions");
    }
    "/v1/chat/completions".to_string()
}

/// Convert Anthropic /v1/messages JSON response to OpenAI chat/completions shape.
pub fn anthropic_response_to_openai(body: &Value) -> Value {
    if body.get("choices").is_some() {
        return body.clone();
    }
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    if let Some(content) = body.get("content").and_then(|c| c.as_array()) {
        for b in content {
            match b.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                "text" => text.push_str(b.get("text").and_then(|t| t.as_str()).unwrap_or("")),
                "tool_use" => tool_calls.push(serde_json::json!({
                    "id": b.get("id").cloned().unwrap_or(serde_json::json!("tool")),
                    "type": "function",
                    "function": {
                        "name": b.get("name").cloned().unwrap_or(serde_json::json!("")),
                        "arguments": b.get("input").cloned().unwrap_or(serde_json::json!({})).to_string()
                    }
                })),
                _ => {}
            }
        }
    }
    let mut message = serde_json::json!({"role": "assistant", "content": text});
    if !tool_calls.is_empty() {
        message["tool_calls"] = serde_json::json!(tool_calls);
    }
    serde_json::json!({
        "id": body.get("id").cloned().unwrap_or(serde_json::json!("smr-converted")),
        "object": "chat.completion",
        "model": body.get("model").cloned().unwrap_or(serde_json::json!("")),
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": body.get("stop_reason").cloned().unwrap_or(serde_json::json!("stop"))
        }]
    })
}

/// Convert OpenAI chat/completions response to Anthropic message shape.
pub fn openai_response_to_anthropic(body: &Value) -> Value {
    if body.get("content").is_some() && body.get("role").is_some() {
        return body.clone();
    }
    let mut blocks = Vec::new();
    if let Some(choices) = body.get("choices").and_then(|c| c.as_array()) {
        if let Some(msg) = choices.first().and_then(|c| c.get("message")) {
            if let Some(text) = msg.get("content").and_then(|c| c.as_str()) {
                if !text.is_empty() {
                    blocks.push(serde_json::json!({"type": "text", "text": text}));
                }
            }
            if let Some(tool_calls) = msg.get("tool_calls").and_then(|t| t.as_array()) {
                for tc in tool_calls {
                    let args = tc
                        .get("function")
                        .and_then(|f| f.get("arguments"))
                        .and_then(|a| a.as_str())
                        .unwrap_or("{}");
                    let input: Value = serde_json::from_str(args).unwrap_or(serde_json::json!(args));
                    blocks.push(serde_json::json!({
                        "type": "tool_use",
                        "id": tc.get("id").cloned().unwrap_or(serde_json::json!("tool")),
                        "name": tc.get("function").and_then(|f| f.get("name")).cloned().unwrap_or(serde_json::json!("")),
                        "input": input
                    }));
                }
            }
        }
    }
    serde_json::json!({
        "id": body.get("id").cloned().unwrap_or(serde_json::json!("smr-converted")),
        "type": "message",
        "role": "assistant",
        "model": body.get("model").cloned().unwrap_or(serde_json::json!("")),
        "content": blocks,
        "stop_reason": "end_turn"
    })
}

/// Convert a single SSE JSON event between provider streaming formats (text deltas).
pub fn convert_sse_chunk(chunk: &Value, from: ApiProtocol, to: ApiProtocol) -> Value {
    if from == to {
        return chunk.clone();
    }
    match (from, to) {
        (ApiProtocol::Anthropic, ApiProtocol::OpenAi) => anthropic_sse_chunk_to_openai(chunk),
        (ApiProtocol::OpenAi, ApiProtocol::Anthropic) => openai_sse_chunk_to_anthropic(chunk),
        _ => chunk.clone(),
    }
}

fn anthropic_sse_chunk_to_openai(chunk: &Value) -> Value {
    match chunk.get("type").and_then(|t| t.as_str()) {
        Some("content_block_delta") => {
            if let Some(text) = chunk
                .get("delta")
                .and_then(|d| d.get("text"))
                .and_then(|t| t.as_str())
            {
                return json!({
                    "choices": [{"index": 0, "delta": {"content": text}}]
                });
            }
        }
        Some("message_stop") => {
            return json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]});
        }
        _ => {}
    }
    chunk.clone()
}

fn openai_sse_chunk_to_anthropic(chunk: &Value) -> Value {
    if let Some(choice) = chunk
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|c| c.first())
    {
        if let Some(content) = choice
            .get("delta")
            .and_then(|d| d.get("content"))
            .and_then(|c| c.as_str())
        {
            return json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": content}
            });
        }
        if choice.get("finish_reason").and_then(|f| f.as_str()) == Some("stop") {
            return json!({"type": "message_stop"});
        }
    }
    chunk.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ApiProtocol;
    use serde_json::json;

    #[test]
    fn converts_basic_openai_request() {
        let body = json!({
            "model": "claude",
            "messages": [
                {"role": "system", "content": "You are helpful"},
                {"role": "user", "content": "hi"}
            ]
        });
        let out = openai_to_anthropic(&body);
        assert_eq!(out["system"], "You are helpful");
        assert_eq!(out["messages"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn converts_openai_sse_text_delta_to_anthropic() {
        let chunk = json!({"choices":[{"delta":{"content":"Hi"}}]});
        let out = convert_sse_chunk(&chunk, ApiProtocol::OpenAi, ApiProtocol::Anthropic);
        assert_eq!(out["type"], "content_block_delta");
        assert_eq!(out["delta"]["text"], "Hi");
    }

    #[test]
    fn converts_anthropic_sse_text_delta_to_openai() {
        let chunk = json!({
            "type": "content_block_delta",
            "delta": {"type": "text_delta", "text": "Hi"}
        });
        let out = convert_sse_chunk(&chunk, ApiProtocol::Anthropic, ApiProtocol::OpenAi);
        assert_eq!(out["choices"][0]["delta"]["content"], "Hi");
    }
}
