#[cfg(test)]
mod reversible_tests {
    use crate::config::{
        AppConfig, ContentCategory, ContentRule, MatchMode, PipelineConfig, ServerConfig,
    };
    use crate::dlp::DlpEngine;
    use serde_json::json;
    use smr_protocol::{extract_texts, inject_response_texts};

    fn test_config_with_secret() -> AppConfig {
        AppConfig {
            server: ServerConfig::default(),
            pipeline: PipelineConfig {
                dlp_enabled: true,
                dlp_reversible: true,
                ..Default::default()
            },
            logging: Default::default(),
            fallback_groups: Default::default(),
            content_rules: vec![ContentRule {
                id: "pw".into(),
                enabled: true,
                match_mode: MatchMode::Full,
                value: "ssh-secret-pass".into(),
                category: ContentCategory::Secret,
                min_fragment_len: None,
                min_fragment_ratio: None,
            }],
            file_rules: vec![],
            operation_rules: vec![],
            path_protection_rules: vec![],
        }
    }

    #[test]
    fn request_redacts_with_token_and_response_restores_tool_call() {
        let cfg = test_config_with_secret();
        let dlp = DlpEngine::new(&cfg).unwrap();
        let session = "sess-restore";

        let request = json!({
            "messages": [{"role": "user", "content": "login with password ssh-secret-pass"}]
        });
        let extracted = extract_texts(&request).unwrap();
        let (req_repl, _) = dlp
            .process_request(session, &extracted, &request, false)
            .unwrap();
        assert!(!req_repl.is_empty());
        let mut forward = request.clone();
        smr_protocol::inject_texts(&mut forward, &req_repl).unwrap();
        let user = forward["messages"][0]["content"].as_str().unwrap();
        assert!(!user.contains("ssh-secret-pass"));
        assert!(user.contains("[[smr:"));

        let token = dlp.vault().token_for(session, "ssh-secret-pass");
        let response = json!({
            "choices": [{
                "message": {
                    "tool_calls": [{
                        "function": {
                            "arguments": format!(r#"{{"password":"{token}"}}"#)
                        }
                    }]
                }
            }]
        });
        let resp_extracted = extract_texts(&response).unwrap();
        let (resp_repl, _) = dlp.process_response(session, &response, &resp_extracted).unwrap();
        assert!(!resp_repl.is_empty());
        let mut client_resp = response.clone();
        inject_response_texts(&mut client_resp, &resp_repl).unwrap();
        let args = client_resp["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .unwrap();
        assert!(args.contains("ssh-secret-pass"));
    }

    #[test]
    fn assistant_text_is_not_restored() {
        let cfg = test_config_with_secret();
        let dlp = DlpEngine::new(&cfg).unwrap();
        let session = "sess-no-restore-text";
        let token = dlp.vault().token_for(session, "ssh-secret-pass");

        let response = json!({
            "choices": [{
                "message": {
                    "content": format!("use token {token} only")
                }
            }]
        });
        let extracted = extract_texts(&response).unwrap();
        let (repl, _) = dlp.process_response(session, &response, &extracted).unwrap();
        assert!(repl.is_empty() || repl.iter().all(|(_, t)| t.contains("[[smr:")));
    }
}
