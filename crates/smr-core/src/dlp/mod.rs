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

use crate::config::{AppConfig, UiLanguage};
use parking_lot::RwLock;
use smr_protocol::{
    extract_tool_call_texts, is_model_input, is_tool_result_content,
    ExtractedText,
};
use std::path::PathBuf;

pub struct DlpEngine {
    content: RwLock<ContentDlp>,
    file: FileDlp,
    sessions: SessionGuard,
    vault: TokenVault,
    enabled: RwLock<bool>,
    reversible: RwLock<bool>,
    ui_language: RwLock<UiLanguage>,
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
        Self::with_sessions_vault_and_file_index(config, sessions, vault, None)
    }

    /// Test / isolated runs: file index under `index_root` instead of the global config dir.
    pub fn with_file_index_root(config: &AppConfig, index_root: PathBuf) -> anyhow::Result<Self> {
        Self::with_sessions_vault_and_file_index(
            config,
            SessionGuard::new(),
            TokenVault::new(),
            Some(index_root),
        )
    }

    fn with_sessions_vault_and_file_index(
        config: &AppConfig,
        sessions: SessionGuard,
        vault: TokenVault,
        file_index_root: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        let enabled = config.pipeline.dlp_active();
        let reversible = config.pipeline.dlp_reversible;
        let file = match file_index_root {
            Some(root) => FileDlp::with_index_root(root, &config.file_rules)?,
            None => FileDlp::new(&config.file_rules)?,
        };
        Ok(Self {
            content: RwLock::new(ContentDlp::new(&config.content_rules, &config.pipeline)?),
            file,
            sessions,
            vault,
            enabled: RwLock::new(enabled),
            reversible: RwLock::new(reversible),
            ui_language: RwLock::new(config.server.ui_language),
        })
    }

    pub fn sync_runtime_config(&self, config: &AppConfig) {
        *self.ui_language.write() = config.server.ui_language;
    }

    fn tool_output_block_message(&self) -> String {
        self.ui_language
            .read()
            .file_tool_output_block_message()
            .to_string()
    }

    pub fn vault(&self) -> &TokenVault {
        &self.vault
    }

    pub fn sessions(&self) -> &SessionGuard {
        &self.sessions
    }

    pub fn reload(&self, config: &AppConfig) -> anyhow::Result<()> {
        self.sync_runtime_config(config);
        *self.content.write() = ContentDlp::new(&config.content_rules, &config.pipeline)?;
        *self.enabled.write() = config.pipeline.dlp_active();
        *self.reversible.write() = config.pipeline.dlp_reversible;
        self.file.reload(&config.file_rules)
    }

    pub fn is_file_index_ready(&self) -> bool {
        self.file.is_index_ready() && !self.file.is_index_rebuilding()
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
    ) -> anyhow::Result<(Vec<(ExtractedText, String)>, usize, bool)> {
        let sid = session_id.to_string();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.process_request_inner(&sid, extracted, request_json, reboost_windows)
        })) {
            Ok(result) => match result {
                Ok(v) => Ok(v),
                Err(e) => {
                    tracing::error!(
                        session_id = %sid,
                        error = %e,
                        "DLP request processing failed; forwarding without DLP"
                    );
                    Ok((Vec::new(), 0, false))
                }
            },
            Err(_) => {
                tracing::error!(
                    session_id = %sid,
                    "DLP request processing panicked; forwarding without DLP"
                );
                Ok((Vec::new(), 0, false))
            }
        }
    }

    fn process_request_inner(
        &self,
        session_id: &str,
        extracted: &[ExtractedText],
        request_json: &serde_json::Value,
        reboost_windows: bool,
    ) -> anyhow::Result<(Vec<(ExtractedText, String)>, usize, bool)> {
        if !*self.enabled.read() {
            return Ok((Vec::new(), 0, false));
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
        let mut needs_system_notice = false;
        let dlp_notice_already_sent = self.sessions.dlp_system_notice_sent(session_id);
        for item in extracted {
            let scan_files = is_model_input(item, request_json);
            let whole_block = scan_files && is_tool_result_content(item, request_json);
            let sanitized = self.redact_for_model(
                session_id,
                &item.text,
                session_active.as_deref(),
                scan_files,
                whole_block,
            )?;
            if sanitized != item.text {
                replacements.push((item.clone(), sanitized.clone()));
                if !dlp_notice_already_sent
                    && !needs_system_notice
                    && self.replacement_requires_system_notice(
                        item,
                        &item.text,
                        &sanitized,
                        request_json,
                    )
                {
                    needs_system_notice = true;
                }
            }
        }
        if needs_system_notice {
            self.sessions.mark_dlp_system_notice_sent(session_id);
        }
        let count = replacements.len();
        Ok((replacements, count, needs_system_notice))
    }

    /// Response-side: restore tool-call fields; redact other fields that still contain secrets.
    pub fn process_response(
        &self,
        session_id: &str,
        response_json: &serde_json::Value,
        extracted: &[ExtractedText],
    ) -> anyhow::Result<(Vec<(ExtractedText, String)>, usize)> {
        let sid = session_id.to_string();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.process_response_inner(&sid, response_json, extracted)
        })) {
            Ok(result) => match result {
                Ok(v) => Ok(v),
                Err(e) => {
                    tracing::error!(
                        session_id = %sid,
                        error = %e,
                        "DLP response processing failed; forwarding without DLP"
                    );
                    Ok((Vec::new(), 0))
                }
            },
            Err(_) => {
                tracing::error!(
                    session_id = %sid,
                    "DLP response processing panicked; forwarding without DLP"
                );
                Ok((Vec::new(), 0))
            }
        }
    }

    fn process_response_inner(
        &self,
        session_id: &str,
        response_json: &serde_json::Value,
        extracted: &[ExtractedText],
    ) -> anyhow::Result<(Vec<(ExtractedText, String)>, usize)> {
        if !*self.enabled.read() {
            return Ok((Vec::new(), 0));
        }

        self.apply_path_triggers(session_id, response_json);

        let session_active = self.sessions.active_snapshot(session_id);

        let mut replacements = Vec::new();
        for item in extracted {
            let scan_files = is_model_input(item, response_json);
            let new_text = if *self.reversible.read()
                && smr_protocol::is_tool_related(item, response_json)
            {
                self.vault.restore(session_id, &item.text)
            } else {
                let whole_block = scan_files && is_tool_result_content(item, response_json);
                self.redact_for_model(
                    session_id,
                    &item.text,
                    session_active.as_deref(),
                    scan_files,
                    whole_block,
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
        whole_block_on_match: bool,
    ) -> anyhow::Result<String> {
        let content = self.content.read();
        let content_protected = content.has_protected_content(text);
        let reversible = *self.reversible.read();
        let sanitized = if reversible {
            content.sanitize_text_reversible(text, session_id, &self.vault)?
        } else {
            content.sanitize_text(text)?
        };
        drop(content);

        // Api-key / password / secret / content-rule hits: span-level redaction only.
        // Skip file DLP on the same text so surrounding task context stays intact and
        // reversible tokens can restore in tool-call arguments.
        if content_protected {
            return Ok(sanitized);
        }

        if scan_files {
            if let Some(active) = session_active {
                let block_message = self.tool_output_block_message();
                // Whole-block tool output only for pure file-index hits; api-key / password /
                // secret / content rules stay span-level (see `content_protected` early return).
                let file_whole_block = whole_block_on_match && !content_protected;
                let file_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    self.sessions.sanitize_with_active(
                        &sanitized,
                        active,
                        &self.file,
                        if reversible {
                            Some((session_id, &self.vault))
                        } else {
                            None
                        },
                        file_whole_block,
                        &block_message,
                    )
                }));
                match file_result {
                    Ok(out) => Ok(out),
                    Err(_) => {
                        tracing::error!(
                            session_id,
                            "file DLP scan panicked; using content-only sanitization"
                        );
                        Ok(sanitized)
                    }
                }
            } else {
                Ok(sanitized)
            }
        } else {
            Ok(sanitized)
        }
    }

    fn apply_path_triggers(&self, session_id: &str, body: &serde_json::Value) {
        let tool_args = match collect_tool_call_trigger_text(body) {
            Some(s) if !s.is_empty() => s,
            _ => return,
        };
        self.file
            .check_path_triggers_in_tool_text(session_id, &tool_args, |sid, rule, files| {
                self.sessions.activate(sid, rule, files, rule.trigger_window);
            });
    }

    /// True when upstream should receive an extra system notice (excludes reversible tool-arg tokens).
    fn replacement_requires_system_notice(
        &self,
        item: &ExtractedText,
        old: &str,
        new: &str,
        request_json: &serde_json::Value,
    ) -> bool {
        if is_tool_result_content(item, request_json) {
            return true;
        }
        if !*self.reversible.read() {
            return true;
        }
        if is_pure_reversible_token_substitution(old, new) {
            return false;
        }
        true
    }
}

/// `new` differs from `old` only by replacing contiguous spans with `[[smr:…]]` tokens.
fn is_pure_reversible_token_substitution(old: &str, new: &str) -> bool {
    use regex::Regex;
    use std::sync::OnceLock;

    static TOKEN_SPLIT: OnceLock<Regex> = OnceLock::new();
    let re = TOKEN_SPLIT.get_or_init(|| Regex::new(r"\[\[smr:[0-9a-f]{8}\]\]").expect("token re"));
    if !re.is_match(new) {
        return false;
    }
    let mut rest = old;
    for part in re.split(new) {
        if part.is_empty() {
            continue;
        }
        let Some(pos) = rest.find(part) else {
            return false;
        };
        rest = &rest[pos + part.len()..];
    }
    true
}

/// Tool-call / tool_use arguments only — used for path triggers (not tool-result listings).
pub(crate) fn collect_tool_call_trigger_text(body: &serde_json::Value) -> Option<String> {
    let parts: Vec<String> = extract_tool_call_texts(body)
        .ok()?
        .into_iter()
        .map(|t| t.text)
        .filter(|t| !t.is_empty())
        .collect();
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
        AppConfig, FileRule, LoggingConfig, MatchMode, PipelineConfig, ServerConfig, UiLanguage,
    };
    use smr_protocol::extract_texts;
    use std::fs;
    use std::time::Duration;
    use tempfile::TempDir;

    /// File DLP tests must not write indexes under the real config dir (sandbox / stale hydrate).
    struct IsolatedFileDlp {
        _index_root: TempDir,
        engine: DlpEngine,
    }

    impl IsolatedFileDlp {
        fn new(config: &AppConfig) -> Self {
            let index_root = TempDir::new().expect("temp file-index dir");
            let engine =
                DlpEngine::with_file_index_root(config, index_root.path().to_path_buf())
                    .expect("dlp engine");
            engine.reload(config).expect("dlp reload");
            wait_file_index_ready(&engine);
            Self {
                _index_root: index_root,
                engine,
            }
        }
    }

    fn wait_file_index_ready(dlp: &DlpEngine) {
        for _ in 0..400 {
            if dlp.is_file_index_ready() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(dlp.is_file_index_ready(), "file index not ready");
    }

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
            insight: Default::default(),
        };

        let harness = IsolatedFileDlp::new(&config);
        let dlp = &harness.engine;

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
        let (repl, count, _) = dlp.process_request(session, &extracted2, &leak, false)
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
    fn protected_directory_ls_does_not_activate_session() {
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
            insight: Default::default(),
        };

        let harness = IsolatedFileDlp::new(&config);
        let dlp = &harness.engine;

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
            "directory-only ls must not activate file DLP"
        );
    }

    #[test]
    fn cd_ls_directory_only_does_not_activate_session() {
        let tmp = TempDir::new().unwrap();
        let zone = tmp.path().join("protected-zone");
        fs::create_dir_all(&zone).unwrap();
        let long_name = "Annual_Report_For_Activation_Test.pdf";
        let report = zone.join(long_name);
        fs::write(&report, "Q".repeat(65)).unwrap();

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
                match_mode: MatchMode::Fragment,
                min_fragment_len: None,
                min_fragment_ratio: None,
                formats: vec!["pdf".into(), "txt".into()],
                index: Default::default(),
            }],
            operation_rules: vec![],
            path_protection_rules: vec![],
            insight: Default::default(),
        };

        let harness = IsolatedFileDlp::new(&config);
        let dlp = &harness.engine;

        let zone_str = zone.to_string_lossy().replace('\\', "/");
        let session = "ls-listing";
        let listing = format!("total 8\n-rw-r--r-- 1 user staff 4096 Jan 1 00:00 {long_name}\n");
        let trigger = serde_json::json!({
            "messages": [
                {"role": "assistant", "content": null, "tool_calls": [{
                    "id": "c1",
                    "type": "function",
                    "function": {
                        "name": "exec",
                        "arguments": format!(r#"{{"command":"cd \"{zone_str}\" && ls -la"}}"#)
                    }
                }]},
                {"role": "tool", "tool_call_id": "c1", "content": listing}
            ]
        });
        dlp.register_path_triggers(session, &trigger);
        assert!(
            dlp.sessions().active_snapshot(session).is_none(),
            "cd + directory-only ls must not activate file DLP"
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
            insight: Default::default(),
        };

        let harness = IsolatedFileDlp::new(&config);
        let dlp = &harness.engine;

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
        let (repl, count, notice) = dlp.process_request(session, &extracted, &request, false)
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

    #[test]
    fn pdftotext_command_with_comma_path_triggers_and_redacts() {
        let tmp = TempDir::new().unwrap();
        let secret = "X".repeat(80);
        let fname = "Aibaba, Question Directed Graph Attention Network for Numerical Reasoning over Text.pdf";
        let zone = tmp.path().join("Table-NLP");
        fs::create_dir_all(&zone).unwrap();
        let pdf = zone.join(fname);
        fs::write(&pdf, format!("{secret}\n\nChapter 1 body")).unwrap();
        let pdf_path = pdf.to_string_lossy().replace('\\', "/");

        let config = AppConfig {
            server: ServerConfig {
                ui_language: UiLanguage::Zh,
                ..Default::default()
            },
            pipeline: PipelineConfig {
                dlp_enabled: true,
                ..Default::default()
            },
            logging: LoggingConfig::default(),
            fallback_groups: Default::default(),
            content_rules: vec![],
            file_rules: vec![FileRule {
                id: "table-nlp".into(),
                path: zone.clone(),
                enabled: true,
                recursive: true,
                trigger_window: 15,
                match_mode: MatchMode::Full,
                min_fragment_len: None,
                min_fragment_ratio: None,
                formats: vec!["pdf".into(), "txt".into()],
                index: Default::default(),
            }],
            operation_rules: vec![],
            path_protection_rules: vec![],
            insight: Default::default(),
        };

        let harness = IsolatedFileDlp::new(&config);
        let dlp = &harness.engine;

        let session = "openclaw-pdftotext";
        let command = format!(
            r#"pdftotext -f 1 -l 20 "{pdf_path}" - 2>/dev/null | head -300"#
        );
        let request = serde_json::json!({
            "messages": [
                {"role": "user", "content": "read chapter 1"},
                {"role": "assistant", "content": null, "tool_calls": [{
                    "id": "c1",
                    "type": "function",
                    "function": {
                        "name": "exec",
                        "arguments": serde_json::json!({ "command": command }).to_string()
                    }
                }]},
                {"role": "tool", "tool_call_id": "c1", "content": format!("{secret}\nchapter one")}
            ]
        });

        dlp.register_path_triggers(session, &request);
        assert!(
            dlp.sessions().active_snapshot(session).is_some(),
            "pdftotext command path should activate file DLP"
        );

        let extracted = extract_texts(&request).unwrap();
        let (repl, count, notice) = dlp.process_request(session, &extracted, &request, false)
            .unwrap();
        assert!(count > 0, "expected file DLP replacements");
        let expected = UiLanguage::Zh.file_tool_output_block_message();
        let sanitized = repl
            .iter()
            .find(|(item, text)| item.text.contains(&secret) && *text == expected)
            .map(|(_, text)| text.clone())
            .unwrap_or_else(|| repl.first().map(|(_, t)| t.clone()).unwrap_or_default());
        assert_eq!(
            sanitized, expected,
            "file-only tool output should be wholly replaced, got: {sanitized}"
        );
        assert!(
            !sanitized.contains(&secret),
            "tool result should be redacted: {sanitized}"
        );
    }

    #[test]
    fn postscript_pdf_stream_passes_when_no_fragment_match() {
        let tmp = TempDir::new().unwrap();
        let secret = "X".repeat(80);
        let fname = "Deep Learning For Anomaly Detection - A Survey, Sydney.pdf";
        let zone = tmp.path().join("Surveys");
        fs::create_dir_all(&zone).unwrap();
        let pdf = zone.join(fname);
        fs::write(&pdf, format!("{secret}\n\nAbstract body text")).unwrap();

        let config = AppConfig {
            server: ServerConfig {
                ui_language: UiLanguage::Zh,
                ..Default::default()
            },
            pipeline: PipelineConfig {
                dlp_enabled: true,
                ..Default::default()
            },
            logging: LoggingConfig::default(),
            fallback_groups: Default::default(),
            content_rules: vec![],
            file_rules: vec![FileRule {
                id: "surveys".into(),
                path: zone.clone(),
                enabled: true,
                recursive: true,
                trigger_window: 15,
                match_mode: MatchMode::Fragment,
                min_fragment_len: Some(65),
                min_fragment_ratio: Some(0.5),
                formats: vec!["pdf".into(), "txt".into()],
                index: Default::default(),
            }],
            operation_rules: vec![],
            path_protection_rules: vec![],
            insight: Default::default(),
        };

        let harness = IsolatedFileDlp::new(&config);
        let dlp = &harness.engine;

        let session = "postscript-bypass";
        let zone_str = zone.to_string_lossy().replace('\\', "/");
        let pdf_path = pdf.to_string_lossy().replace('\\', "/");
        let postscript = "BT /F45 17 Tf [(D)]TJ/F45 13 Tf [(E)-61(A)-62(R)-62(N)-62(I)-62(N)-61(G)]TJ ET";
        let request = serde_json::json!({
            "messages": [
                {"role": "user", "content": "read pdf"},
                {"role": "assistant", "content": null, "tool_calls": [{
                    "id": "c1",
                    "type": "function",
                    "function": {
                        "name": "exec",
                        "arguments": format!(r#"{{"command":"python3 -c \"open('{pdf_path}')\""}}"#)
                    }
                }]},
                {"role": "tool", "tool_call_id": "c1", "content": postscript}
            ]
        });

        dlp.register_path_triggers(session, &request);
        assert!(
            dlp.sessions().active_snapshot(session).is_some(),
            "pdf path in exec should activate file DLP session"
        );
        let extracted = extract_texts(&request).unwrap();
        let (repl, count, _notice) = dlp.process_request(session, &extracted, &request, false)
            .unwrap();
        let postscript_out = repl
            .iter()
            .find(|(item, _)| item.text.contains("]TJ"))
            .map(|(_, t)| t.as_str())
            .unwrap_or_else(|| {
                extracted
                    .iter()
                    .find(|e| e.text.contains("]TJ"))
                    .map(|e| e.text.as_str())
                    .unwrap_or("")
            });
        assert_eq!(
            postscript_out,
            postscript,
            "PostScript without indexed fragment match should pass through (replacements={count}, repl={repl:?})"
        );
    }

    #[test]
    fn hexdump_and_partial_script_pass_without_fragment_match() {
        let tmp = TempDir::new().unwrap();
        let secret = "X".repeat(80);
        let fname = "openclaw_surveys_dlp_verify.py";
        let zone = tmp.path().join("scripts");
        fs::create_dir_all(&zone).unwrap();
        let script = zone.join(fname);
        fs::write(
            &script,
            format!("#!/usr/bin/env python3\n\"\"\"secret module\"\"\"\n{secret}\n"),
        )
        .unwrap();

        let config = AppConfig {
            server: ServerConfig {
                ui_language: UiLanguage::Zh,
                ..Default::default()
            },
            pipeline: PipelineConfig {
                dlp_enabled: true,
                ..Default::default()
            },
            logging: LoggingConfig::default(),
            fallback_groups: Default::default(),
            content_rules: vec![],
            file_rules: vec![FileRule {
                id: "scripts".into(),
                path: zone.clone(),
                enabled: true,
                recursive: true,
                trigger_window: 15,
                match_mode: MatchMode::Fragment,
                min_fragment_len: Some(65),
                min_fragment_ratio: Some(0.5),
                formats: vec!["py".into()],
                index: Default::default(),
            }],
            operation_rules: vec![],
            path_protection_rules: vec![],
            insight: Default::default(),
        };

        let harness = IsolatedFileDlp::new(&config);
        let dlp = &harness.engine;

        let session = "scripts-hex-bypass";
        let script_path = script.to_string_lossy().replace('\\', "/");
        let partial = "#!/usr/bin/env python3\n\"\"\"secret module\"\"\"\n";
        let hexdump = "00000000  23 21 2f 75 73 72 2f 62  69 6e 2f 65 6e 76 20 70  |#!/usr/bin/env p|";
        let request = serde_json::json!({
            "messages": [
                {"role": "user", "content": "read script"},
                {"role": "assistant", "content": null, "tool_calls": [{
                    "id": "c1",
                    "type": "function",
                    "function": {
                        "name": "exec",
                        "arguments": format!(r#"{{"command":"perl -ne 'print' \"{script_path}\""}}"#)
                    }
                }]},
                {"role": "tool", "tool_call_id": "c1", "content": partial},
                {"role": "assistant", "content": null, "tool_calls": [{
                    "id": "c2",
                    "type": "function",
                    "function": {
                        "name": "exec",
                        "arguments": format!(r#"{{"command":"hexdump -C \"{script_path}\""}}"#)
                    }
                }]},
                {"role": "tool", "tool_call_id": "c2", "content": hexdump}
            ]
        });

        dlp.register_path_triggers(session, &request);
        let extracted = extract_texts(&request).unwrap();
        let (repl, _, _notice) = dlp.process_request(session, &extracted, &request, false)
            .unwrap();
        for label in ["partial", "hexdump"] {
            let needle = if label == "partial" { "#!/usr/bin/env" } else { "00000000" };
            let out = repl
                .iter()
                .find(|(item, _)| item.text.contains(needle))
                .map(|(_, t)| t.as_str())
                .unwrap_or_else(|| {
                    extracted
                        .iter()
                        .find(|e| e.text.contains(needle))
                        .map(|e| e.text.as_str())
                        .unwrap_or("")
                });
            assert!(
                out.contains(needle),
                "{label} tool output should pass through without indexed fragment match"
            );
        }
    }

    /// Replays a captured OpenClaw traffic body against the live user config + file index.
    #[test]
    fn repro_openclaw_understanding_tables_traffic() {
        use crate::config::AppConfig;
        use crate::paths::{config_dir, default_config_path};
        use std::path::PathBuf;

        let traffic_path = config_dir().join("traffic/20260610T144445_request_in_fabcb12f.body");
        if !traffic_path.exists() {
            eprintln!("skip: traffic snapshot not found at {}", traffic_path.display());
            return;
        }
        let config = AppConfig::load(&default_config_path()).expect("load smr.yaml");
        let body: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&traffic_path).unwrap()).unwrap();

        let dlp = DlpEngine::new(&config).unwrap();
        dlp.reload(&config).unwrap();
        for _ in 0..600 {
            if dlp.is_file_index_ready() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            dlp.is_file_index_ready(),
            "file index not ready for repro"
        );

        let session = "openclaw-traffic-repro";
        dlp.register_path_triggers(session, &body);
        let active = dlp.sessions().active_snapshot(session);
        eprintln!("session active: {:?}", active.as_ref().map(|a| a.len()));
        if let Some(a) = &active {
            for item in a {
                eprintln!(
                    "  rule={} triggered_files={:?}",
                    item.rule.id, item.triggered_files
                );
            }
        }
        assert!(
            active.is_some(),
            "expected path trigger from pdftotext exec in traffic body"
        );

        let extracted = extract_texts(&body).unwrap();
        let tool_items: Vec<_> = extracted
            .iter()
            .filter(|e| {
                smr_protocol::is_model_input(e, &body)
                    && e.text.len() > 1000
                    && e.text.contains("Understanding tables")
            })
            .collect();
        eprintln!("model-input tool-like fields: {}", tool_items.len());

        let (repl, count, notice) = dlp.process_request(session, &extracted, &body, false).unwrap();
        eprintln!("fragment mode dlp replacements count: {count}");

        // Same traffic with Full match mode (isolates fragment/normalization issues).
        let mut full_config = config.clone();
        for rule in &mut full_config.file_rules {
            if rule.id == "file-1781067561965" {
                rule.match_mode = MatchMode::Full;
                rule.min_fragment_len = None;
                rule.min_fragment_ratio = None;
            }
        }
        let dlp_full = DlpEngine::new(&full_config).unwrap();
        dlp_full.reload(&full_config).unwrap();
        for _ in 0..600 {
            if dlp_full.is_file_index_ready() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        dlp_full.register_path_triggers(session, &body);
        let (repl_full, count_full, _) =
            dlp_full.process_request(session, &extracted, &body, false).unwrap();
        eprintln!("full mode dlp replacements count: {count_full}");
        if count_full > 0 {
            if let Some((item, text)) = repl_full.iter().find(|(i, _)| {
                i.text.len() > 1000 && i.text.contains("Understanding tables")
            }) {
                eprintln!(
                    "  full mode redacted len {} -> {}",
                    item.text.len(),
                    text.len()
                );
            }
        }

        assert!(
            count > 0,
            "expected fragment-mode DLP to redact PDF tool result (full mode count={count_full})"
        );
    }

    #[test]
    fn repro_patterson_cod_page100_traffic() {
        use crate::config::AppConfig;
        use crate::dlp::file::{extract_triggered_files, FileDlp};
        use crate::paths::{config_dir, default_config_path};
        use smr_protocol::extract_texts;

        let traffic_path = config_dir().join("traffic/20260617T101727_request_in_b0aa2763.body");
        if !traffic_path.exists() {
            eprintln!("skip: Patterson traffic snapshot missing");
            return;
        }
        let config = AppConfig::load(&default_config_path()).expect("load smr.yaml");
        let body: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&traffic_path).unwrap()).unwrap();

        let dlp = DlpEngine::new(&config).unwrap();
        dlp.reload(&config).unwrap();
        for _ in 0..600 {
            if dlp.is_file_index_ready() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(dlp.is_file_index_ready(), "file index not ready");

        let tool_blob = super::collect_tool_call_trigger_text(&body).unwrap_or_default();
        let rule = config
            .file_rules
            .iter()
            .find(|r| r.id == "file-1781662177086")
            .expect("patterson rule");
        let candidates = extract_triggered_files(&tool_blob, rule);
        assert!(
            candidates.iter().any(|p| p.contains("Third Edition, Revised.pdf")),
            "expected fitz tool call to mention COD PDF path"
        );

        let fdlp = FileDlp::new(&config.file_rules).unwrap();
        fdlp.reload(&config.file_rules).unwrap();
        for _ in 0..600 {
            if fdlp.is_index_ready() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let activated = std::cell::RefCell::new(Vec::<String>::new());
        fdlp.check_path_triggers_in_tool_text("sess", &tool_blob, |_, _, files| {
            activated.borrow_mut().extend(files.iter().cloned());
        });
        assert!(
            !activated.into_inner().is_empty(),
            "path trigger must resolve against on-disk manifest paths"
        );
        assert!(
            !fdlp.indexed_paths_for_rule("file-1781662177086").is_empty(),
            "indexed_paths must not be empty when reusing a valid on-disk generation"
        );

        let session = "patterson-repro";
        dlp.register_path_triggers(session, &body);
        assert!(
            dlp.sessions()
                .active_snapshot(session)
                .is_some_and(|a| !a.is_empty()),
            "session should activate from fitz.open path in tool calls"
        );

        let extracted = extract_texts(&body).unwrap();
        let (_repl, count, _notice) =
            dlp.process_request(session, &extracted, &body, false).unwrap();
        eprintln!(
            "patterson repro: session ok; dlp replacements={count} (non-zero after index rebuild with dense fragment signatures)"
        );
    }

    /// Win2 physical-machine capture: read D:\\long.txt tool result must not panic DLP.
    #[test]
    fn repro_win2_long_txt_traffic() {
        use std::path::PathBuf;
        use std::time::Instant;

        let win2 = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/test-logs/securemodelroute-Win2");
        let traffic = win2.join("traffic/20260621T210320_request_in_824a3461.body");
        if !traffic.exists() {
            eprintln!("skip: Win2 traffic fixture missing at {}", traffic.display());
            return;
        }
        let config = AppConfig::load(&win2.join("smr.yaml")).expect("config");
        let body: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&traffic).unwrap()).unwrap();
        let dlp = DlpEngine::with_file_index_root(&config, win2.join("file-index")).expect("dlp");
        dlp.reload(&config).expect("reload");
        for _ in 0..200 {
            if dlp.is_file_index_ready() {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(dlp.is_file_index_ready(), "file index not ready");

        let session = "win2-long-txt-repro";
        dlp.register_path_triggers(session, &body);
        assert!(
            dlp.sessions()
                .active_snapshot(session)
                .is_some_and(|a| !a.is_empty()),
            "expected D:/long.txt path trigger from read tool call"
        );

        let extracted = extract_texts(&body).unwrap();
        let t0 = Instant::now();
        let (repl, count, notice) = dlp
            .process_request(session, &extracted, &body, false)
            .expect("process_request must not error after P1 degrade");
        eprintln!(
            "win2 long.txt repro: {:?} repl={} notice={}",
            t0.elapsed(),
            count,
            notice
        );
        let _ = repl;
    }

    /// Win3 physical-machine capture: path trigger + file DLP on pdf page-100 tool result.
    #[test]
    fn repro_win3_pdf_page100_traffic() {
        use std::cell::Cell;
        use std::path::PathBuf;

        let win3 = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/test-logs/securemodelroute-win3");
        let traffic = win3.join("traffic/20260621T232804_request_in_4f6019b8.body");
        if !traffic.exists() {
            eprintln!("skip: Win3 traffic fixture missing at {}", traffic.display());
            return;
        }
        let config = AppConfig::load(&win3.join("smr.yaml")).expect("config");
        let body: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&traffic).unwrap()).unwrap();
        let dlp = DlpEngine::with_file_index_root(&config, win3.join("file-index")).expect("dlp");
        dlp.reload(&config).expect("reload");
        for _ in 0..400 {
            if dlp.is_file_index_ready() {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(dlp.is_file_index_ready(), "file index not ready");

        let session = "win3-pdf-page100-repro";
        let tool_blob = smr_protocol::extract_tool_call_texts(&body)
            .unwrap()
            .into_iter()
            .map(|t| t.text)
            .collect::<Vec<_>>()
            .join("\n");
        let rule = config
            .file_rules
            .iter()
            .find(|r| r.id == "file-1782050783999")
            .expect("dlp rule");
        let path_candidates = crate::dlp::file::extract_triggered_files(&tool_blob, rule);
        eprintln!("win3: extract_triggered_files={}", path_candidates.len());
        for p in path_candidates.iter().take(5) {
            eprintln!("  candidate: {p}");
        }

        let file_dlp =
            crate::dlp::file::FileDlp::with_index_root(win3.join("file-index"), &config.file_rules)
                .expect("file dlp");
        file_dlp.reload(&config.file_rules).expect("file reload");
        let activated = Cell::new(0usize);
        file_dlp.check_path_triggers_in_tool_text(session, &tool_blob, |_, r, files| {
            activated.set(activated.get() + 1);
            eprintln!("win3: activate rule={} files={}", r.id, files.len());
            for f in files.iter().take(5) {
                eprintln!("  triggered: {f}");
            }
        });
        eprintln!("win3: check_path_triggers activations={}", activated.get());

        dlp.register_path_triggers(session, &body);
        let active = dlp.sessions().active_snapshot(session);
        eprintln!(
            "win3: active rules={}",
            active.as_ref().map(|a| a.len()).unwrap_or(0)
        );
        if let Some(a) = &active {
            for item in a {
                eprintln!(
                    "  rule={} triggered_files={}",
                    item.rule.id,
                    item.triggered_files.len()
                );
            }
        }

        let extracted = extract_texts(&body).unwrap();
        let page_tool = body["messages"]
            .as_array()
            .and_then(|m| m.get(81))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("");
        eprintln!("win3: page-100 tool msg len={}", page_tool.len());

        let (repl, count, notice) = dlp
            .process_request(session, &extracted, &body, false)
            .expect("process_request");
        eprintln!("win3: repl={count} notice={notice}");

        let mut page_redacted = false;
        for (old, new) in &repl {
            if old.text.contains("Page number: undefined") {
                page_redacted = old.text != *new;
                eprintln!(
                    "win3: page-100 field redacted={} old_len={} new_len={}",
                    page_redacted,
                    old.text.len(),
                    new.len()
                );
            }
        }
        eprintln!(
            "win3: SUMMARY path_trigger_ok={} file_dlp_redacted={}",
            activated.get() > 0,
            page_redacted
        );

        assert!(
            activated.get() > 0,
            "path normalization should enable Win3 path triggers"
        );
    }
}

#[cfg(test)]
mod content_reload_tests {
    use super::*;
    use crate::config::{
        AppConfig, ContentCategory, ContentRule, LoggingConfig, MatchMode, PipelineConfig,
        ServerConfig,
    };
    use smr_protocol::extract_texts;

    #[test]
    fn reload_applies_new_content_rules_without_restart() {
        let mut config = AppConfig {
            server: ServerConfig::default(),
            pipeline: PipelineConfig {
                dlp_enabled: true,
                dlp_reversible: true,
                ..Default::default()
            },
            logging: LoggingConfig::default(),
            fallback_groups: Default::default(),
            content_rules: vec![],
            file_rules: vec![],
            operation_rules: vec![],
            path_protection_rules: vec![],
            insight: Default::default(),
        };

        let dlp = DlpEngine::new(&config).unwrap();
        let body = serde_json::json!({
            "messages": [{"role": "user", "content": "my password is Sky@420117"}]
        });
        let extracted = extract_texts(&body).unwrap();
        let (_, count_before, _) = dlp.process_request("s1", &extracted, &body, false).unwrap();
        assert_eq!(count_before, 0, "no rules yet");

        config.content_rules.push(ContentRule {
            id: "secret-test".into(),
            enabled: true,
            match_mode: MatchMode::Full,
            value: "Sky@420117".into(),
            category: ContentCategory::Secret,
            min_fragment_len: None,
            min_fragment_ratio: None,
        });
        dlp.reload(&config).unwrap();

        let (_, count_after, _) = dlp.process_request("s1", &extracted, &body, false).unwrap();
        assert_eq!(count_after, 1, "reload should pick up new content rule");
    }
}
