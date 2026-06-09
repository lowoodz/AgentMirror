mod bloom;
mod charset;
mod content;
mod disk_index;
mod doc_extract;
mod file;
mod shell_paths;
mod fragment;
mod normalize;
mod rg;
mod sanitize;
mod session;
mod vault;

pub use content::ContentDlp;
pub use file::FileDlp;
pub use session::SessionGuard;
pub use vault::TokenVault;

use crate::config::AppConfig;
use smr_protocol::{extract_tool_call_texts, is_model_input, is_tool_related, ExtractedText};

pub struct DlpEngine {
    content: ContentDlp,
    file: FileDlp,
    sessions: SessionGuard,
    vault: TokenVault,
    enabled: bool,
    reversible: bool,
}

impl DlpEngine {
    pub fn new(config: &AppConfig) -> anyhow::Result<Self> {
        Self::with_sessions(config, SessionGuard::new())
    }

    pub fn with_sessions(config: &AppConfig, sessions: SessionGuard) -> anyhow::Result<Self> {
        Self::with_sessions_and_vault(config, sessions, TokenVault::new())
    }

    pub fn with_sessions_and_vault(
        config: &AppConfig,
        sessions: SessionGuard,
        vault: TokenVault,
    ) -> anyhow::Result<Self> {
        let enabled = config.pipeline.dlp_active();
        let reversible = config.pipeline.dlp_reversible;
        Ok(Self {
            content: ContentDlp::new(&config.content_rules, &config.pipeline)?,
            file: FileDlp::new(&config.file_rules)?,
            sessions,
            vault,
            enabled,
            reversible,
        })
    }

    pub fn vault(&self) -> &TokenVault {
        &self.vault
    }

    pub fn sessions(&self) -> &SessionGuard {
        &self.sessions
    }

    pub fn reload(&self, config: &AppConfig) -> anyhow::Result<()> {
        self.file.reload(&config.file_rules)
    }

    pub fn is_file_index_ready(&self) -> bool {
        self.file.is_index_ready()
    }

    pub fn is_file_index_rebuilding(&self) -> bool {
        self.file.is_index_rebuilding()
    }

    /// Register file-path session triggers from tool calls (call before ops may rewrite arguments).
    pub fn register_path_triggers(&self, session_id: &str, body: &serde_json::Value) {
        self.apply_path_triggers(session_id, body);
    }

    pub fn process_request(
        &self,
        session_id: &str,
        extracted: &[ExtractedText],
        request_json: &serde_json::Value,
        reboost_windows: bool,
    ) -> anyhow::Result<(Vec<(ExtractedText, String)>, usize)> {
        if !self.enabled {
            return Ok((Vec::new(), 0));
        }

        self.apply_path_triggers(session_id, request_json);
        let mut session_active = self.sessions.begin_request(session_id);
        if reboost_windows {
            self.sessions.reboost_windows(session_id);
            if session_active.is_none() {
                session_active = self.sessions.active_snapshot(session_id);
            }
        }
        let mut replacements = Vec::new();
        for item in extracted {
            let scan_files = is_model_input(item, request_json);
            let sanitized = self.redact_for_model(
                session_id,
                &item.text,
                session_active.as_deref(),
                scan_files,
            )?;
            if sanitized != item.text {
                replacements.push((item.clone(), sanitized));
            }
        }
        let count = replacements.len();
        Ok((replacements, count))
    }

    /// Response-side: restore tool-call fields; redact other fields that still contain secrets.
    pub fn process_response(
        &self,
        session_id: &str,
        response_json: &serde_json::Value,
        extracted: &[ExtractedText],
    ) -> anyhow::Result<(Vec<(ExtractedText, String)>, usize)> {
        if !self.enabled {
            return Ok((Vec::new(), 0));
        }

        self.apply_path_triggers(session_id, response_json);

        let session_active = self.sessions.active_snapshot(session_id);

        let mut replacements = Vec::new();
        for item in extracted {
            let scan_files = is_model_input(item, response_json);
            let new_text = if self.reversible && is_tool_related(item, response_json) {
                self.vault.restore(session_id, &item.text)
            } else {
                self.redact_for_model(
                    session_id,
                    &item.text,
                    session_active.as_deref(),
                    scan_files,
                )?
            };
            if new_text != item.text {
                replacements.push((item.clone(), new_text));
            }
        }
        let count = replacements.len();
        Ok((replacements, count))
    }

    fn redact_for_model(
        &self,
        session_id: &str,
        text: &str,
        session_active: Option<&[session::ActiveFileContent]>,
        scan_files: bool,
    ) -> anyhow::Result<String> {
        let sanitized = if self.reversible {
            self.content
                .sanitize_text_reversible(text, session_id, &self.vault)?
        } else {
            self.content.sanitize_text(text)?
        };
        if scan_files {
            if let Some(active) = session_active {
                Ok(self.sessions.sanitize_with_active(
                    &sanitized,
                    active,
                    &self.file,
                    if self.reversible {
                        Some((session_id, &self.vault))
                    } else {
                        None
                    },
                ))
            } else {
                Ok(sanitized)
            }
        } else {
            Ok(sanitized)
        }
    }

    fn apply_path_triggers(&self, session_id: &str, body: &serde_json::Value) {
        let tool_blob = match collect_tool_path_scan_text(body) {
            Some(s) if !s.is_empty() => s,
            _ => return,
        };
        self.file
            .check_path_triggers_in_tool_text(session_id, &tool_blob, |sid, rule, files| {
                self.sessions.activate(sid, rule, files, rule.trigger_window);
            });
    }
}

/// Tool-call arguments plus tool-result bodies — any mention of a protected zone activates DLP.
fn collect_tool_path_scan_text(body: &serde_json::Value) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Ok(tool_texts) = extract_tool_call_texts(body) {
        for t in tool_texts {
            if !t.text.is_empty() {
                parts.push(t.text);
            }
        }
    }
    if let Some(messages) = body.get("messages").and_then(|m| m.as_array()) {
        for msg in messages {
            if msg.get("role").and_then(|r| r.as_str()) != Some("tool") {
                continue;
            }
            match msg.get("content") {
                Some(serde_json::Value::String(s)) if !s.is_empty() => parts.push(s.clone()),
                Some(serde_json::Value::Array(blocks)) => {
                    for block in blocks {
                        if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                            if !t.is_empty() {
                                parts.push(t.to_string());
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

#[cfg(test)]
mod reversible_tests;

#[cfg(test)]
mod file_session_tests {
    use super::*;
    use crate::config::{
        AppConfig, FileRule, LoggingConfig, MatchMode, PipelineConfig, ServerConfig,
    };
    use smr_protocol::extract_texts;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn session_trigger_then_scan_redacts_file_content() {
        let tmp = TempDir::new().unwrap();
        let secret = "P".repeat(65);
        let probe = tmp.path().join("probe.txt");
        fs::write(&probe, &secret).unwrap();

        let config = AppConfig {
            server: ServerConfig::default(),
            pipeline: PipelineConfig {
                dlp_enabled: true,
                ..Default::default()
            },
            logging: LoggingConfig::default(),
            fallback_groups: Default::default(),
            content_rules: vec![],
            file_rules: vec![FileRule {
                id: "t1".into(),
                path: tmp.path().to_path_buf(),
                enabled: true,
                recursive: true,
                trigger_window: 5,
                match_mode: MatchMode::Full,
                min_fragment_len: None,
                min_fragment_ratio: None,
                formats: vec!["txt".into()],
                index: Default::default(),
            }],
            operation_rules: vec![],
            path_protection_rules: vec![],
        };

        let dlp = DlpEngine::new(&config).unwrap();
        dlp.reload(&config).unwrap();
        assert!(dlp.is_file_index_ready(), "file index not ready");

        let session = "test-sess";
        let probe_path = probe.to_string_lossy().replace('\\', "/");

        let trigger = serde_json::json!({
            "messages": [
                {"role": "user", "content": "read file"},
                {"role": "assistant", "content": null, "tool_calls": [{
                    "id": "c1",
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "arguments": format!(r#"{{"path":"{probe_path}"}}"#)
                    }
                }]}
            ]
        });
        let tool_blob = smr_protocol::extract_tool_call_texts(&trigger)
            .unwrap()
            .iter()
            .map(|t| t.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        dlp.register_path_triggers(session, &trigger);
        assert!(
            dlp.sessions().active_snapshot(session).is_some(),
            "path trigger should activate session; tool_blob={tool_blob:?}"
        );
        let extracted = extract_texts(&trigger).unwrap();
        dlp.process_request(session, &extracted, &trigger, false)
            .unwrap();

        let leak = serde_json::json!({
            "messages": [{"role": "user", "content": format!("leak {secret}")}]
        });
        let extracted2 = extract_texts(&leak).unwrap();
        let (repl, count) = dlp.process_request(session, &extracted2, &leak, false)
            .unwrap();

        assert!(count > 0, "expected file DLP replacements");
        let sanitized = repl
            .first()
            .map(|(_, t)| t.as_str())
            .unwrap_or(&extracted2[0].text);
        assert!(
            !sanitized.contains(&secret),
            "file secret should be redacted: {sanitized}"
        );
    }

    #[test]
    fn protected_directory_mention_does_not_activate_session() {
        let tmp = TempDir::new().unwrap();
        let probe = tmp.path().join("probe.txt");
        fs::write(&probe, "Q".repeat(65)).unwrap();

        let config = AppConfig {
            server: ServerConfig::default(),
            pipeline: PipelineConfig {
                dlp_enabled: true,
                ..Default::default()
            },
            logging: LoggingConfig::default(),
            fallback_groups: Default::default(),
            content_rules: vec![],
            file_rules: vec![FileRule {
                id: "zone".into(),
                path: tmp.path().to_path_buf(),
                enabled: true,
                recursive: true,
                trigger_window: 15,
                match_mode: MatchMode::Fragment,
                min_fragment_len: None,
                min_fragment_ratio: None,
                formats: vec!["txt".into()],
                index: Default::default(),
            }],
            operation_rules: vec![],
            path_protection_rules: vec![],
        };

        let dlp = DlpEngine::new(&config).unwrap();
        dlp.reload(&config).unwrap();
        for _ in 0..300 {
            if dlp.is_file_index_ready() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(dlp.is_file_index_ready());

        let dir = tmp.path().to_string_lossy().replace('\\', "/");
        let session = "zone-ls";
        let trigger = serde_json::json!({
            "messages": [{
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "c1",
                    "type": "function",
                    "function": {
                        "name": "exec",
                        "arguments": format!(r#"{{"command":"ls -la \"{dir}\""}}"#)
                    }
                }]
            }]
        });
        dlp.register_path_triggers(session, &trigger);
        assert!(
            dlp.sessions().active_snapshot(session).is_none(),
            "directory-only mention must not activate file DLP"
        );
    }

    #[test]
    fn exec_cd_relative_path_triggers_and_redacts_tool_result() {
        let tmp = TempDir::new().unwrap();
        let secret = "P".repeat(65);
        let zone = tmp.path().join("protected-zone");
        fs::create_dir_all(&zone).unwrap();
        let probe = zone.join("thesis.txt");
        fs::write(&probe, &secret).unwrap();

        let config = AppConfig {
            server: ServerConfig::default(),
            pipeline: PipelineConfig {
                dlp_enabled: true,
                ..Default::default()
            },
            logging: LoggingConfig::default(),
            fallback_groups: Default::default(),
            content_rules: vec![],
            file_rules: vec![FileRule {
                id: "zone".into(),
                path: zone.clone(),
                enabled: true,
                recursive: true,
                trigger_window: 15,
                match_mode: MatchMode::Full,
                min_fragment_len: None,
                min_fragment_ratio: None,
                formats: vec!["txt".into()],
                index: Default::default(),
            }],
            operation_rules: vec![],
            path_protection_rules: vec![],
        };

        let dlp = DlpEngine::new(&config).unwrap();
        dlp.reload(&config).unwrap();
        for _ in 0..300 {
            if dlp.is_file_index_ready() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(dlp.is_file_index_ready());

        let zone_str = zone.to_string_lossy().replace('\\', "/");
        let session = "exec-cd-session";
        let command = format!(r#"cd "{zone_str}" && cat "thesis.txt""#);
        let request = serde_json::json!({
            "messages": [
                {"role": "user", "content": "read thesis"},
                {"role": "assistant", "content": null, "tool_calls": [{
                    "id": "c1",
                    "type": "function",
                    "function": {
                        "name": "exec",
                        "arguments": serde_json::json!({ "command": command }).to_string()
                    }
                }]},
                {"role": "tool", "tool_call_id": "c1", "content": secret.clone()}
            ]
        });

        let extracted = extract_texts(&request).unwrap();
        let (repl, count) = dlp.process_request(session, &extracted, &request, false)
            .unwrap();

        assert!(count > 0, "expected file DLP replacements on tool result");
        let tool_sanitized = repl
            .iter()
            .find(|(item, _)| item.text == secret)
            .map(|(_, text)| text.as_str())
            .or_else(|| {
                repl.iter()
                    .find(|(item, text)| *text != item.text)
                    .map(|(_, text)| text.as_str())
            })
            .unwrap_or("");
        assert!(
            !tool_sanitized.contains(&secret),
            "tool result should be redacted: {tool_sanitized}"
        );
    }
}
