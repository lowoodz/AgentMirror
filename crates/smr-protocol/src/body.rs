use anyhow::{Context, Result};
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct ExtractedText {
    pub pointer: TextPointer,
    pub text: String,
}

#[derive(Debug, Clone)]
pub enum TextPointer {
    OpenAiMessageContent { message_index: usize },
    OpenAiMessageString { message_index: usize },
    OpenAiToolCallArguments { message_index: usize, tool_index: usize },
    OpenAiDeltaToolCallArguments { choice_index: usize, tool_index: usize },
    AnthropicContentBlock { message_index: usize, block_index: usize },
}

/// Extract all textual fields from a request or response body for scanning.
pub fn extract_texts(body: &Value) -> Result<Vec<ExtractedText>> {
    let mut out = Vec::new();

    if let Some(messages) = body.get("messages").and_then(|m| m.as_array()) {
        for (mi, msg) in messages.iter().enumerate() {
            extract_openai_message(msg, mi, &mut out);

            if let Some(content) = msg.get("content").and_then(|c| c.as_array()) {
                for (bi, block) in content.iter().enumerate() {
                    extract_anthropic_block(block, mi, bi, &mut out);
                }
            }
        }
    }

    if let Some(choices) = body.get("choices").and_then(|c| c.as_array()) {
        for (ci, choice) in choices.iter().enumerate() {
            if let Some(msg) = choice.get("message") {
                extract_openai_message(msg, 0, &mut out);
            }
            if let Some(text) = choice.get("text").and_then(|t| t.as_str()) {
                out.push(ExtractedText {
                    pointer: TextPointer::OpenAiMessageString { message_index: 0 },
                    text: text.to_string(),
                });
            }
            if let Some(delta) = choice.get("delta") {
                if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                    for (ti, tc) in tool_calls.iter().enumerate() {
                        if let Some(args) = tc
                            .get("function")
                            .and_then(|f| f.get("arguments"))
                            .and_then(|a| a.as_str())
                        {
                            out.push(ExtractedText {
                                pointer: TextPointer::OpenAiDeltaToolCallArguments {
                                    choice_index: ci,
                                    tool_index: ti,
                                },
                                text: args.to_string(),
                            });
                        }
                    }
                }
            }
        }
    }

    if let Some(content) = body.get("content").and_then(|c| c.as_array()) {
        for (bi, block) in content.iter().enumerate() {
            extract_anthropic_block(block, 0, bi, &mut out);
        }
    }

    Ok(out)
}

/// Extract only tool-call / tool-result fields (for path triggers and response-side session guard).
pub fn extract_tool_call_texts(body: &Value) -> Result<Vec<ExtractedText>> {
    let all = extract_texts(body)?;
    Ok(filter_tool_related(body, &all))
}

/// Tool-related pointers: OpenAI tool args, Anthropic tool_use / tool_result blocks.
pub fn is_tool_related(extracted: &ExtractedText, body: &Value) -> bool {
    match &extracted.pointer {
        TextPointer::OpenAiToolCallArguments { .. }
        | TextPointer::OpenAiDeltaToolCallArguments { .. } => true,
        TextPointer::AnthropicContentBlock {
            message_index,
            block_index,
        } => {
            if let Some(blocks) = body
                .get("messages")
                .and_then(|m| m.get(*message_index))
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            {
                return blocks.get(*block_index).is_some_and(|b| {
                    matches!(
                        b.get("type").and_then(|t| t.as_str()),
                        Some("tool_use") | Some("tool_result")
                    )
                });
            }
            if let Some(blocks) = body.get("content").and_then(|c| c.as_array()) {
                return blocks.get(*block_index).is_some_and(|b| {
                    matches!(
                        b.get("type").and_then(|t| t.as_str()),
                        Some("tool_use") | Some("tool_result")
                    )
                });
            }
            false
        }
        _ => false,
    }
}

pub fn filter_tool_related(body: &Value, extracted: &[ExtractedText]) -> Vec<ExtractedText> {
    extracted
        .iter()
        .filter(|e| is_tool_related(e, body))
        .cloned()
        .collect()
}

/// Tool result bodies sent back to the model (OpenAI `role: tool`, Anthropic `tool_result`).
pub fn is_tool_result_content(extracted: &ExtractedText, body: &Value) -> bool {
    match &extracted.pointer {
        TextPointer::OpenAiMessageString { message_index }
        | TextPointer::OpenAiMessageContent { message_index } => {
            message_role(body, *message_index) == Some("tool")
        }
        TextPointer::AnthropicContentBlock {
            message_index,
            block_index,
        } => anthropic_block(body, *message_index, *block_index)
            .and_then(|b| b.get("type"))
            .and_then(|t| t.as_str())
            == Some("tool_result"),
        _ => false,
    }
}

/// Fields that are input to the model/agent (excludes assistant/model-generated text).
pub fn is_model_input(extracted: &ExtractedText, body: &Value) -> bool {
    match &extracted.pointer {
        TextPointer::OpenAiMessageString { message_index }
        | TextPointer::OpenAiMessageContent { message_index } => {
            message_role(body, *message_index) != Some("assistant")
        }
        TextPointer::OpenAiToolCallArguments { message_index, .. } => {
            message_role(body, *message_index) != Some("assistant")
        }
        TextPointer::OpenAiDeltaToolCallArguments { .. } => false,
        TextPointer::AnthropicContentBlock {
            message_index,
            block_index,
        } => anthropic_block_is_model_input(body, *message_index, *block_index),
    }
}

pub fn filter_model_input(body: &Value, extracted: &[ExtractedText]) -> Vec<ExtractedText> {
    extracted
        .iter()
        .filter(|e| is_model_input(e, body))
        .cloned()
        .collect()
}

fn message_role(body: &Value, message_index: usize) -> Option<&str> {
    body.get("messages")
        .and_then(|m| m.get(message_index))
        .and_then(|m| m.get("role"))
        .and_then(|r| r.as_str())
}

fn anthropic_block_is_model_input(body: &Value, message_index: usize, block_index: usize) -> bool {
    let role = message_role(body, message_index);
    if role == Some("assistant") {
        return false;
    }
    if role == Some("user") || role == Some("system") {
        return true;
    }
    if body.get("messages").is_none() {
        // Response bodies use top-level `content` with implicit assistant role.
        return false;
    }
    anthropic_block(body, message_index, block_index)
        .and_then(|b| b.get("type"))
        .and_then(|t| t.as_str())
        .is_some_and(|t| matches!(t, "text" | "tool_result"))
}

fn anthropic_block<'a>(body: &'a Value, message_index: usize, block_index: usize) -> Option<&'a Value> {
    body.get("messages")
        .and_then(|m| m.get(message_index))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
        .and_then(|blocks| blocks.get(block_index))
        .or_else(|| {
            body.get("content")
                .and_then(|c| c.as_array())
                .and_then(|blocks| blocks.get(block_index))
        })
}

fn extract_openai_message(msg: &Value, mi: usize, out: &mut Vec<ExtractedText>) {
    match msg.get("content") {
        Some(Value::String(s)) => {
            out.push(ExtractedText {
                pointer: TextPointer::OpenAiMessageString { message_index: mi },
                text: s.clone(),
            });
        }
        Some(Value::Array(parts)) => {
            let mut combined = String::new();
            for part in parts {
                if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                    combined.push_str(text);
                }
            }
            if !combined.is_empty() {
                out.push(ExtractedText {
                    pointer: TextPointer::OpenAiMessageContent { message_index: mi },
                    text: combined,
                });
            }
        }
        _ => {}
    }

    if let Some(tool_calls) = msg.get("tool_calls").and_then(|t| t.as_array()) {
        for (ti, tc) in tool_calls.iter().enumerate() {
            if let Some(args) = tc
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(|a| a.as_str())
            {
                out.push(ExtractedText {
                    pointer: TextPointer::OpenAiToolCallArguments {
                        message_index: mi,
                        tool_index: ti,
                    },
                    text: args.to_string(),
                });
            }
        }
    }
}

fn extract_anthropic_block(block: &Value, mi: usize, bi: usize, out: &mut Vec<ExtractedText>) {
    let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match block_type {
        "text" => {
            if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                out.push(ExtractedText {
                    pointer: TextPointer::AnthropicContentBlock {
                        message_index: mi,
                        block_index: bi,
                    },
                    text: text.to_string(),
                });
            }
        }
        "tool_use" => {
            if let Some(input) = block.get("input") {
                out.push(ExtractedText {
                    pointer: TextPointer::AnthropicContentBlock {
                        message_index: mi,
                        block_index: bi,
                    },
                    text: input.to_string(),
                });
            }
        }
        "tool_result" => {
            let content = block
                .get("content")
                .map(|c| {
                    if let Some(s) = c.as_str() {
                        s.to_string()
                    } else {
                        c.to_string()
                    }
                })
                .unwrap_or_default();
            if !content.is_empty() {
                out.push(ExtractedText {
                    pointer: TextPointer::AnthropicContentBlock {
                        message_index: mi,
                        block_index: bi,
                    },
                    text: content,
                });
            }
        }
        _ => {}
    }
}

/// Write sanitized texts back into the JSON body (request).
pub fn inject_texts(body: &mut Value, replacements: &[(ExtractedText, String)]) -> Result<()> {
    for (extracted, new_text) in replacements {
        match &extracted.pointer {
            TextPointer::OpenAiMessageString { message_index } => {
                body["messages"][*message_index]["content"] = json!(new_text);
            }
            TextPointer::OpenAiMessageContent { message_index } => {
                if let Some(parts) = body["messages"][*message_index]["content"].as_array_mut() {
                    if let Some(first) = parts.first_mut() {
                        first["text"] = json!(new_text);
                    }
                }
            }
            TextPointer::OpenAiToolCallArguments {
                message_index,
                tool_index,
            } => {
                body["messages"][*message_index]["tool_calls"][*tool_index]["function"]["arguments"] =
                    json!(new_text);
            }
            TextPointer::OpenAiDeltaToolCallArguments {
                choice_index,
                tool_index,
            } => {
                if let Some(choices) = body.get_mut("choices").and_then(|c| c.as_array_mut()) {
                    if let Some(delta) = choices
                        .get_mut(*choice_index)
                        .and_then(|c| c.get_mut("delta"))
                    {
                        delta["tool_calls"][*tool_index]["function"]["arguments"] =
                            json!(new_text);
                    }
                }
            }
            TextPointer::AnthropicContentBlock {
                message_index,
                block_index,
            } => {
                if body.get("messages").is_some() {
                    let block = &mut body["messages"][*message_index]["content"][*block_index];
                    inject_anthropic_block(block, new_text);
                } else {
                    let block = &mut body["content"][*block_index];
                    inject_anthropic_block(block, new_text);
                }
            }
        }
    }
    Ok(())
}

fn inject_anthropic_block(block: &mut Value, new_text: &str) {
    let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match block_type {
        "text" => block["text"] = json!(new_text),
        "tool_use" => {
            if let Ok(parsed) = serde_json::from_str::<Value>(new_text) {
                block["input"] = parsed;
            } else {
                block["input"] = json!(new_text);
            }
        }
        "tool_result" => block["content"] = json!(new_text),
        _ => {}
    }
}

/// Write sanitized texts back into response bodies.
pub fn inject_response_texts(body: &mut Value, replacements: &[(ExtractedText, String)]) -> Result<()> {
    for (extracted, new_text) in replacements {
        match &extracted.pointer {
            TextPointer::OpenAiMessageString { .. }
            | TextPointer::OpenAiMessageContent { .. } => {
                if let Some(choices) = body.get_mut("choices").and_then(|c| c.as_array_mut()) {
                    if let Some(msg) = choices.first_mut().and_then(|c| c.get_mut("message")) {
                        msg["content"] = json!(new_text);
                    }
                }
            }
            TextPointer::OpenAiToolCallArguments { tool_index, .. } => {
                if let Some(choices) = body.get_mut("choices").and_then(|c| c.as_array_mut()) {
                    if let Some(msg) = choices.first_mut().and_then(|c| c.get_mut("message")) {
                        msg["tool_calls"][*tool_index]["function"]["arguments"] = json!(new_text);
                    }
                }
            }
            TextPointer::OpenAiDeltaToolCallArguments {
                choice_index,
                tool_index,
            } => {
                if let Some(choices) = body.get_mut("choices").and_then(|c| c.as_array_mut()) {
                    if let Some(delta) = choices
                        .get_mut(*choice_index)
                        .and_then(|c| c.get_mut("delta"))
                    {
                        delta["tool_calls"][*tool_index]["function"]["arguments"] =
                            json!(new_text);
                    }
                }
            }
            TextPointer::AnthropicContentBlock { block_index, .. } => {
                if let Some(content) = body.get_mut("content").and_then(|c| c.as_array_mut()) {
                    inject_anthropic_block(&mut content[*block_index], new_text);
                }
            }
        }
    }
    Ok(())
}

pub fn parse_json_body(bytes: &[u8]) -> Result<Value> {
    serde_json::from_slice(bytes).context("invalid JSON body")
}

pub fn serialize_json_body(value: &Value) -> Result<Vec<u8>> {
    serde_json::to_vec(value).context("failed to serialize JSON body")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_delta_tool_call_arguments() {
        let body = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": {"arguments": r#"{"command":"rm -rf /"}"#}
                    }]
                }
            }]
        });
        let extracted = extract_texts(&body).unwrap();
        let tools: Vec<_> = extracted
            .iter()
            .filter(|e| matches!(e.pointer, TextPointer::OpenAiDeltaToolCallArguments { .. }))
            .collect();
        assert_eq!(tools.len(), 1);
        assert!(tools[0].text.contains("rm -rf"));
        assert!(is_tool_related(tools[0], &body));
    }

    #[test]
    fn text_blocks_not_marked_tool_related_when_tool_calls_present() {
        let body = json!({
            "choices": [{
                "message": {
                    "content": "plain answer",
                    "tool_calls": [{"function": {"arguments": "{}"}}]
                }
            }],
            "content": [{"type": "text", "text": "plain answer"}]
        });
        let extracted = extract_texts(&body).unwrap();
        let text_blocks: Vec<_> = extracted
            .iter()
            .filter(|e| matches!(e.pointer, TextPointer::AnthropicContentBlock { .. }))
            .collect();
        assert!(!text_blocks.is_empty());
        for block in text_blocks {
            assert!(!is_tool_related(block, &body));
        }
    }

    #[test]
    fn assistant_messages_are_not_model_input() {
        let body = json!({
            "messages": [
                {"role": "user", "content": "hello"},
                {"role": "assistant", "content": "world"},
                {"role": "tool", "content": "tool output"}
            ]
        });
        let extracted = extract_texts(&body).unwrap();
        let model_input = filter_model_input(&body, &extracted);
        assert_eq!(model_input.len(), 2);
        assert!(model_input.iter().all(|e| e.text == "hello" || e.text == "tool output"));
    }
}
