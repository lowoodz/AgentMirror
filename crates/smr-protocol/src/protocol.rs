use http::HeaderMap;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiProtocol {
    OpenAi,
    Anthropic,
}

fn body_looks_anthropic(body: &Value) -> bool {
    if body.get("system").is_some() && body.get("messages").is_some() {
        return true;
    }
    body.get("messages")
        .and_then(|m| m.as_array())
        .is_some_and(|arr| {
            arr.iter().any(|msg| {
                msg.get("content")
                    .and_then(|c| c.as_array())
                    .is_some_and(|blocks| {
                        blocks.iter().any(|b| {
                            b.get("type")
                                .and_then(|t| t.as_str())
                                .is_some_and(|t| {
                                    t == "text" || t == "tool_use" || t == "tool_result"
                                })
                        })
                    })
            })
        })
}

fn body_looks_openai(body: &Value) -> bool {
    if body.get("tools").is_some() || body.get("tool_choice").is_some() {
        return true;
    }
    body.get("messages")
        .and_then(|m| m.as_array())
        .is_some_and(|arr| {
            arr.iter().any(|msg| {
                msg.get("tool_calls").is_some()
                    || msg.get("role").and_then(|r| r.as_str()) == Some("tool")
            })
        })
}

fn detect_protocol_from_path(path: &str) -> ApiProtocol {
    let lower = path.to_ascii_lowercase();
    if lower.contains("/messages") && !lower.contains("chat/completions") {
        return ApiProtocol::Anthropic;
    }
    if lower.contains("/chat/completions") || lower.ends_with("/completions") {
        return ApiProtocol::OpenAi;
    }
    ApiProtocol::OpenAi
}

pub fn detect_protocol(path: &str, headers: &HeaderMap, body: &Value) -> ApiProtocol {
    if headers.contains_key("anthropic-version") {
        return ApiProtocol::Anthropic;
    }
    if headers.contains_key("openai-organization") || headers.contains_key("openai-project") {
        return ApiProtocol::OpenAi;
    }

    let anthropic_body = body_looks_anthropic(body);
    let openai_body = body_looks_openai(body);
    if anthropic_body && !openai_body {
        return ApiProtocol::Anthropic;
    }
    if openai_body && !anthropic_body {
        return ApiProtocol::OpenAi;
    }

    detect_protocol_from_path(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderMap;
    use serde_json::json;

    #[test]
    fn detects_openai_path() {
        let body = json!({"messages": []});
        assert_eq!(
            detect_protocol("/v1/chat/completions", &HeaderMap::new(), &body),
            ApiProtocol::OpenAi
        );
    }

    #[test]
    fn detects_anthropic_path() {
        let body = json!({});
        assert_eq!(
            detect_protocol("/v1/messages", &HeaderMap::new(), &body),
            ApiProtocol::Anthropic
        );
    }

    #[test]
    fn detects_anthropic_body_on_openai_path() {
        let body = json!({
            "model": "saferoute-high",
            "max_tokens": 32,
            "system": "You are helpful",
            "messages": [{"role": "user", "content": "hi"}]
        });
        assert_eq!(
            detect_protocol("/v1/chat/completions", &HeaderMap::new(), &body),
            ApiProtocol::Anthropic
        );
    }

    #[test]
    fn detects_openai_tools_on_messages_path() {
        let body = json!({
            "model": "saferoute-high",
            "messages": [{"role": "user", "content": "hi"}],
            "tools": [{"type": "function", "function": {"name": "run", "parameters": {}}}]
        });
        assert_eq!(
            detect_protocol("/v1/messages", &HeaderMap::new(), &body),
            ApiProtocol::OpenAi
        );
    }
}
