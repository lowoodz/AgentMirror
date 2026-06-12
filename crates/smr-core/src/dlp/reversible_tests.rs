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
    fn content_rule_redacts_span_and_preserves_surrounding_task_text() {
        let cfg = test_config_with_secret();
        let dlp = DlpEngine::new(&cfg).unwrap();
        let session = "sess-span-context";

        let tool_body = "Task: write the password ssh-secret-pass to /tmp/out.txt now";
        let request = json!({
            "messages": [
                {"role": "user", "content": "do the write"},
                {"role": "tool", "tool_call_id": "c1", "content": tool_body}
            ]
        });
        let extracted = extract_texts(&request).unwrap();
        let (repl, _) = dlp
            .process_request(session, &extracted, &request, false)
            .unwrap();
        assert!(!repl.is_empty());
        let sanitized = repl
            .iter()
            .find(|(item, _)| item.text == tool_body)
            .map(|(_, t)| t.as_str())
            .unwrap();
        assert!(!sanitized.contains("ssh-secret-pass"));
        assert!(sanitized.contains("[[smr:"));
        assert!(sanitized.contains("Task: write the password"));
        assert!(sanitized.contains("/tmp/out.txt"));
    }

    #[test]
    fn content_rule_skips_file_dlp_on_same_text() {
        use crate::config::{FileIndexOptions, FileRule};
        use std::fs;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let secret = "X".repeat(80);
        let zone = tmp.path().join("zone");
        fs::create_dir_all(&zone).unwrap();
        let probe = zone.join("probe.txt");
        fs::write(&probe, format!("{secret}\nindexed body")).unwrap();

        let cfg = AppConfig {
            server: ServerConfig::default(),
            pipeline: PipelineConfig {
                dlp_enabled: true,
                dlp_reversible: true,
                ..Default::default()
            },
            logging: Default::default(),
            fallback_groups: Default::default(),
            content_rules: vec![ContentRule {
                id: "content-secret".into(),
                enabled: true,
                match_mode: MatchMode::Full,
                value: "SMR-MATRIX-CONTENT-RULE-SECRET".into(),
                category: ContentCategory::Secret,
                min_fragment_len: None,
                min_fragment_ratio: None,
            }],
            file_rules: vec![FileRule {
                id: "zone".into(),
                path: zone.clone(),
                enabled: true,
                recursive: true,
                trigger_window: 5,
                match_mode: MatchMode::Full,
                min_fragment_len: None,
                min_fragment_ratio: None,
                formats: vec!["txt".into()],
                index: FileIndexOptions::default(),
            }],
            operation_rules: vec![],
            path_protection_rules: vec![],
        };

        let dlp = DlpEngine::new(&cfg).unwrap();
        dlp.reload(&cfg).unwrap();
        for _ in 0..400 {
            if dlp.is_file_index_ready() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(dlp.is_file_index_ready());

        let session = "mixed-content-file";
        let probe_path = probe.to_string_lossy().replace('\\', "/");
        let tool_body = format!(
            "Copy SMR-MATRIX-CONTENT-RULE-SECRET to output. File body: {secret}"
        );
        let request = json!({
            "messages": [
                {"role": "user", "content": "write it"},
                {"role": "assistant", "content": null, "tool_calls": [{
                    "id": "c1",
                    "type": "function",
                    "function": {
                        "name": "read",
                        "arguments": json!({ "path": probe_path }).to_string()
                    }
                }]},
                {"role": "tool", "tool_call_id": "c1", "content": tool_body}
            ]
        });

        let extracted = extract_texts(&request).unwrap();
        let (repl, _) = dlp
            .process_request(session, &extracted, &request, false)
            .unwrap();
        let sanitized = repl
            .iter()
            .find(|(item, _)| item.text == tool_body)
            .map(|(_, t)| t.as_str())
            .expect("tool result should be redacted");
        assert!(!sanitized.contains("SMR-MATRIX-CONTENT-RULE-SECRET"));
        assert!(sanitized.contains("[[smr:"));
        assert!(sanitized.contains("Copy "));
        assert!(
            sanitized.contains(&secret),
            "file-indexed body should remain when content rule already matched: {sanitized}"
        );
    }

    #[test]
    fn content_protection_skips_whole_block_even_when_reversible() {
        use crate::config::{FileIndexOptions, FileRule, UiLanguage};
        use std::fs;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let secret = "X".repeat(80);
        let zone = tmp.path().join("zone");
        fs::create_dir_all(&zone).unwrap();
        let probe = zone.join("probe.txt");
        fs::write(&probe, format!("{secret}\nindexed body")).unwrap();

        let cfg = AppConfig {
            server: ServerConfig {
                ui_language: UiLanguage::Zh,
                ..Default::default()
            },
            pipeline: PipelineConfig {
                dlp_enabled: true,
                dlp_reversible: true,
                ..Default::default()
            },
            logging: Default::default(),
            fallback_groups: Default::default(),
            content_rules: vec![ContentRule {
                id: "content-secret".into(),
                enabled: true,
                match_mode: MatchMode::Full,
                value: "SMR-MATRIX-CONTENT-RULE-SECRET".into(),
                category: ContentCategory::Secret,
                min_fragment_len: None,
                min_fragment_ratio: None,
            }],
            file_rules: vec![FileRule {
                id: "zone".into(),
                path: zone.clone(),
                enabled: true,
                recursive: true,
                trigger_window: 5,
                match_mode: MatchMode::Full,
                min_fragment_len: None,
                min_fragment_ratio: None,
                formats: vec!["txt".into()],
                index: FileIndexOptions::default(),
            }],
            operation_rules: vec![],
            path_protection_rules: vec![],
        };

        let dlp = DlpEngine::new(&cfg).unwrap();
        dlp.reload(&cfg).unwrap();
        for _ in 0..400 {
            if dlp.is_file_index_ready() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(dlp.is_file_index_ready());

        let session = "content-not-whole-block";
        let probe_path = probe.to_string_lossy().replace('\\', "/");
        let block = UiLanguage::Zh.file_tool_output_block_message();
        let tool_body = format!(
            "Task: echo SMR-MATRIX-CONTENT-RULE-SECRET. Also file: {secret}"
        );
        let request = json!({
            "messages": [
                {"role": "user", "content": "run"},
                {"role": "assistant", "content": null, "tool_calls": [{
                    "id": "c1",
                    "type": "function",
                    "function": {
                        "name": "read",
                        "arguments": json!({ "path": probe_path }).to_string()
                    }
                }]},
                {"role": "tool", "tool_call_id": "c1", "content": tool_body}
            ]
        });

        let extracted = extract_texts(&request).unwrap();
        let (repl, _) = dlp
            .process_request(session, &extracted, &request, false)
            .unwrap();
        let sanitized = repl
            .iter()
            .find(|(item, _)| item.text == tool_body)
            .map(|(_, t)| t.as_str())
            .expect("tool result should be redacted");
        assert_ne!(sanitized, block);
        assert!(sanitized.contains("Task: echo"));
        assert!(!sanitized.contains("SMR-MATRIX-CONTENT-RULE-SECRET"));
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
