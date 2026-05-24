use http::HeaderMap;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiProtocol {
    OpenAi,
    Anthropic,
}

pub fn detect_protocol(path: &str, headers: &HeaderMap, body: &Value) -> ApiProtocol {
    if path.contains("/messages") {
        return ApiProtocol::Anthropic;
    }
    if path.contains("/chat/completions") {
        return ApiProtocol::OpenAi;
    }

    if headers.contains_key("anthropic-version") {
        return ApiProtocol::Anthropic;
    }
    if headers.contains_key("openai-organization") || headers.contains_key("openai-project") {
        return ApiProtocol::OpenAi;
    }

    if body.get("messages").is_some() {
        if body
            .get("messages")
            .and_then(|m| m.as_array())
            .is_some_and(|arr| {
                arr.iter().any(|msg| {
                    msg.get("content")
                        .and_then(|c| c.as_array())
                        .is_some_and(|blocks| {
                            blocks.iter().any(|b| {
                                b.get("type")
                                    .and_then(|t| t.as_str())
                                    .is_some_and(|t| t == "text" || t == "tool_use" || t == "tool_result")
                            })
                        })
                })
            })
        {
            return ApiProtocol::Anthropic;
        }
        return ApiProtocol::OpenAi;
    }

    if body.get("system").is_some() && body.get("messages").is_some() {
        return ApiProtocol::Anthropic;
    }

    ApiProtocol::OpenAi
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
}
