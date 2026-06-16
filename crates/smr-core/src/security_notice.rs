use smr_protocol::{append_system_message, ExtractedText};

use crate::config::UiLanguage;

/// Append one system notice per ops block category (path vs operation).
pub fn append_ops_system_notices(
    body: &mut serde_json::Value,
    replacements: &[(ExtractedText, String)],
    lang: UiLanguage,
) {
    let mut path = false;
    let mut operation = false;
    for (_, text) in replacements {
        if text.contains("路径防护") || text.contains("path protection") {
            path = true;
        } else if text.contains("SMR BLOCKED") {
            operation = true;
        }
    }
    if path {
        append_system_message(body, lang.path_protection_system_notice());
    }
    if operation {
        append_system_message(body, lang.operation_security_system_notice());
    }
}

pub fn append_dlp_system_notice(body: &mut serde_json::Value, lang: UiLanguage) {
    append_system_message(body, lang.dlp_security_system_notice());
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use smr_protocol::ExtractedText;

    #[test]
    fn ops_notices_split_path_and_operation() {
        let mut body = json!({
            "messages": [{"role": "user", "content": "blocked"}]
        });
        let replacements = vec![
            (
                ExtractedText {
                    pointer: smr_protocol::TextPointer::OpenAiMessageString { message_index: 0 },
                    text: "x".into(),
                },
                "[SMR BLOCKED] path protection".into(),
            ),
            (
                ExtractedText {
                    pointer: smr_protocol::TextPointer::OpenAiMessageString { message_index: 0 },
                    text: "y".into(),
                },
                "[SMR BLOCKED]".into(),
            ),
        ];
        append_ops_system_notices(&mut body, &replacements, UiLanguage::Zh);
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
}
