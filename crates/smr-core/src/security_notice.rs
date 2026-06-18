use crate::config::UiLanguage;
use crate::ops::OpsBlockKinds;
use smr_protocol::append_system_message;

/// Append one system notice per ops block category (path vs operation).
pub fn append_ops_system_notices(
    body: &mut serde_json::Value,
    kinds: OpsBlockKinds,
    lang: UiLanguage,
) {
    if kinds.path {
        append_system_message(body, lang.path_protection_system_notice());
    }
    if kinds.operation {
        append_system_message(body, lang.operation_security_system_notice());
    }
}

pub fn append_dlp_system_notice(body: &mut serde_json::Value, lang: UiLanguage) {
    append_system_message(body, lang.dlp_security_system_notice());
}

/// Prepend ops notices into an OpenAI chat completion assistant message (response JSON).
pub fn prepend_ops_notices_to_completion(
    body: &mut serde_json::Value,
    kinds: OpsBlockKinds,
    lang: UiLanguage,
) {
    if kinds.is_empty() {
        return;
    }
    let mut parts = Vec::new();
    if kinds.path {
        parts.push(lang.path_protection_system_notice());
    }
    if kinds.operation {
        parts.push(lang.operation_security_system_notice());
    }
    let notice = parts.join("\n");
    let Some(message) = body
        .get_mut("choices")
        .and_then(|c| c.as_array_mut())
        .and_then(|c| c.first_mut())
        .and_then(|c| c.get_mut("message"))
    else {
        return;
    };
    let existing = message
        .get("content")
        .and_then(|c| c.as_str())
        .unwrap_or("");
    let merged = if existing.is_empty() {
        notice
    } else {
        format!("{notice}\n\n{existing}")
    };
    message["content"] = serde_json::Value::String(merged);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ops_notices_split_path_and_operation() {
        let mut body = json!({
            "messages": [{"role": "user", "content": "blocked"}]
        });
        append_ops_system_notices(
            &mut body,
            OpsBlockKinds {
                path: true,
                operation: true,
            },
            UiLanguage::Zh,
        );
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert!(messages[1]["content"]
            .as_str()
            .unwrap()
            .contains("路径防护"));
        assert!(messages[2]["content"]
            .as_str()
            .unwrap()
            .contains("操作安全"));
    }

    #[test]
    fn ops_notices_english() {
        let mut body = json!({
            "messages": [{"role": "user", "content": "blocked"}]
        });
        append_ops_system_notices(
            &mut body,
            OpsBlockKinds {
                path: true,
                operation: false,
            },
            UiLanguage::En,
        );
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert!(messages[1]["content"]
            .as_str()
            .unwrap()
            .contains("path protection"));
    }

    #[test]
    fn prepend_ops_notices_into_completion_body() {
        let mut body = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "hello",
                    "tool_calls": []
                }
            }]
        });
        prepend_ops_notices_to_completion(
            &mut body,
            OpsBlockKinds {
                path: true,
                operation: false,
            },
            UiLanguage::Zh,
        );
        let content = body["choices"][0]["message"]["content"]
            .as_str()
            .unwrap();
        assert!(content.contains("重要路径防护"));
        assert!(content.contains("hello"));
    }
}
