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
        for choice in choices {
            if let Some(msg) = choice.get("message") {
                extract_openai_message(msg, 0, &mut out);
            }
            if let Some(text) = choice.get("text").and_then(|t| t.as_str()) {
                out.push(ExtractedText {
                    pointer: TextPointer::OpenAiMessageString { message_index: 0 },
                    text: text.to_string(),
                });
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
