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
}
