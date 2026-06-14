use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct ParsedRequest {
    pub model: Option<String>,
    pub system_excerpt: String,
    pub tools: Vec<String>,
    pub new_messages: Vec<ParsedMessage>,
}

#[derive(Debug, Clone)]
pub struct ParsedMessage {
    pub role: String,
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub tool_results: Vec<ToolResult>,
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub name: String,
    pub content: String,
}

#[derive(Debug, Clone, Default)]
pub struct ParsedResponse {
    pub assistant_text: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
}

pub fn parse_request(body: &[u8]) -> ParsedRequest {
    let Ok(json) = serde_json::from_slice::<Value>(body) else {
        return ParsedRequest::default();
    };
    parse_request_value(&json)
}

pub fn parse_request_value(json: &Value) -> ParsedRequest {
    let model = json.get("model").and_then(|m| m.as_str()).map(str::to_string);
    let tools = extract_tool_names(json.get("tools"));
    let messages = json
        .get("messages")
        .or_else(|| json.get("input"))
        .and_then(|m| m.as_array());

    let mut system_excerpt = String::new();
    let mut new_messages = Vec::new();

    if let Some(msgs) = messages {
        for msg in msgs {
            let role = msg
                .get("role")
                .and_then(|r| r.as_str())
                .unwrap_or("unknown")
                .to_string();
            if role == "system" && system_excerpt.is_empty() {
                system_excerpt = truncate(&message_text(msg), 500);
            }
            new_messages.push(parse_message(msg));
        }
    }

    ParsedRequest {
        model,
        system_excerpt,
        tools,
        new_messages,
    }
}

/// Keep only messages not yet processed for this run (OpenClaw sends full history each turn).
pub fn apply_messages_delta(req: &ParsedRequest, messages_seen: u32) -> ParsedRequest {
    let seen = messages_seen.min(req.new_messages.len() as u32) as usize;
    ParsedRequest {
        model: req.model.clone(),
        system_excerpt: req.system_excerpt.clone(),
        tools: req.tools.clone(),
        new_messages: req.new_messages[seen..].to_vec(),
    }
}

pub fn parse_response(body: &[u8]) -> ParsedResponse {
    let Ok(json) = serde_json::from_slice::<Value>(body) else {
        if looks_like_sse(body) {
            return parse_sse_response(body);
        }
        return ParsedResponse::default();
    };
    parse_response_value(&json)
}

fn parse_response_value(json: &Value) -> ParsedResponse {
    // OpenAI chat completion
    if let Some(choice) = json
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
    {
        let finish = choice
            .get("finish_reason")
            .and_then(|f| f.as_str())
            .map(str::to_string);
        if let Some(msg) = choice.get("message") {
            return ParsedResponse {
                assistant_text: message_text(msg),
                tool_calls: extract_openai_tool_calls(msg),
                finish_reason: finish,
            };
        }
    }

    // Anthropic messages
    if let Some(content) = json.get("content").and_then(|c| c.as_array()) {
        let mut assistant_text = String::new();
        let mut tool_calls = Vec::new();
        for block in content {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if let Some(t) = block.get("text").and_then(|x| x.as_str()) {
                        assistant_text.push_str(t);
                    }
                }
                Some("tool_use") => {
                    tool_calls.push(ToolCall {
                        name: block
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("tool")
                            .to_string(),
                        arguments: block
                            .get("input")
                            .map(|v| v.to_string())
                            .unwrap_or_default(),
                    });
                }
                _ => {}
            }
        }
        let finish = json
            .get("stop_reason")
            .and_then(|f| f.as_str())
            .map(str::to_string);
        return ParsedResponse {
            assistant_text,
            tool_calls,
            finish_reason: finish,
        };
    }

    ParsedResponse::default()
}

fn parse_sse_response(body: &[u8]) -> ParsedResponse {
    let text = String::from_utf8_lossy(body);
    let mut assistant_text = String::new();
    let mut tool_calls = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("data:") {
            continue;
        }
        let payload = trimmed.strip_prefix("data:").unwrap_or("").trim();
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }
        if let Ok(json) = serde_json::from_str::<Value>(payload) {
            if let Some(delta) = json
                .get("choices")
                .and_then(|c| c.as_array())
                .and_then(|a| a.first())
                .and_then(|c| c.get("delta"))
            {
                if let Some(t) = delta.get("content").and_then(|c| c.as_str()) {
                    assistant_text.push_str(t);
                }
                if let Some(calls) = delta.get("tool_calls").and_then(|c| c.as_array()) {
                    for call in calls {
                        if let Some(name) = call
                            .get("function")
                            .and_then(|f| f.get("name"))
                            .and_then(|n| n.as_str())
                        {
                            tool_calls.push(ToolCall {
                                name: name.to_string(),
                                arguments: call
                                    .get("function")
                                    .and_then(|f| f.get("arguments"))
                                    .and_then(|a| a.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                            });
                        }
                    }
                }
            }
        }
    }
    ParsedResponse {
        assistant_text,
        tool_calls,
        finish_reason: None,
    }
}

fn parse_message(msg: &Value) -> ParsedMessage {
    let role = msg
        .get("role")
        .and_then(|r| r.as_str())
        .unwrap_or("unknown")
        .to_string();
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut tool_results = Vec::new();

    match msg.get("content") {
        Some(Value::String(s)) => text = s.clone(),
        Some(Value::Array(blocks)) => {
            for block in blocks {
                match block.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(t) = block.get("text").and_then(|x| x.as_str()) {
                            text.push_str(t);
                        }
                    }
                    Some("tool_use") => {
                        tool_calls.push(ToolCall {
                            name: block
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("tool")
                                .to_string(),
                            arguments: block
                                .get("input")
                                .map(|v| v.to_string())
                                .unwrap_or_default(),
                        });
                    }
                    Some("tool_result") => {
                        tool_results.push(ToolResult {
                            name: block
                                .get("tool_use_id")
                                .and_then(|n| n.as_str())
                                .unwrap_or("tool")
                                .to_string(),
                            content: tool_result_content(block.get("content")),
                        });
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }

    if role == "assistant" && tool_calls.is_empty() {
        tool_calls = extract_openai_tool_calls(msg);
    }
    if role == "tool" {
        tool_results.push(ToolResult {
            name: msg
                .get("name")
                .or_else(|| msg.get("tool_call_id"))
                .and_then(|n| n.as_str())
                .unwrap_or("tool")
                .to_string(),
            content: message_text(msg),
        });
    }

    ParsedMessage {
        role,
        text,
        tool_calls,
        tool_results,
    }
}

fn extract_openai_tool_calls(msg: &Value) -> Vec<ToolCall> {
    msg.get("tool_calls")
        .and_then(|c| c.as_array())
        .map(|calls| {
            calls
                .iter()
                .filter_map(|call| {
                    let name = call
                        .get("function")
                        .and_then(|f| f.get("name"))
                        .and_then(|n| n.as_str())?;
                    Some(ToolCall {
                        name: name.to_string(),
                        arguments: call
                            .get("function")
                            .and_then(|f| f.get("arguments"))
                            .and_then(|a| a.as_str())
                            .unwrap_or("")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn extract_tool_names(tools: Option<&Value>) -> Vec<String> {
    tools
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|tool| {
                    tool.get("function")
                        .and_then(|f| f.get("name"))
                        .and_then(|n| n.as_str())
                        .or_else(|| tool.get("name").and_then(|n| n.as_str()))
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn message_text(msg: &Value) -> String {
    match msg.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

fn tool_result_content(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| {
                item.get("text")
                    .and_then(|t| t.as_str())
                    .or_else(|| item.as_str())
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

fn looks_like_sse(body: &[u8]) -> bool {
    String::from_utf8_lossy(body)
        .lines()
        .any(|l| l.trim().starts_with("data:"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_openai_request() {
        let body = br#"{"model":"gpt-4","messages":[{"role":"user","content":"fix login bug"}],"tools":[{"type":"function","function":{"name":"Read","description":"read"}}]}"#;
        let req = parse_request(body);
        assert_eq!(req.tools, vec!["Read"]);
        assert_eq!(req.new_messages.len(), 1);
        assert!(req.new_messages[0].text.contains("fix login"));
    }

    #[test]
    fn applies_messages_delta() {
        let body = br#"{"messages":[{"role":"user","content":"goal"},{"role":"assistant","content":"ok"},{"role":"tool","content":"data"}]}"#;
        let req = parse_request(body);
        let delta = apply_messages_delta(&req, 2);
        assert_eq!(delta.new_messages.len(), 1);
        assert_eq!(delta.new_messages[0].role, "tool");
    }

    #[test]
    fn parses_openai_response_with_tools() {
        let body = br#"{"choices":[{"message":{"role":"assistant","content":"I'll read the file","tool_calls":[{"id":"1","type":"function","function":{"name":"Read","arguments":"{\"path\":\"a.rs\"}"}}]},"finish_reason":"tool_calls"}]}"#;
        let resp = parse_response(body);
        assert!(resp.assistant_text.contains("read"));
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].name, "Read");
    }
}
