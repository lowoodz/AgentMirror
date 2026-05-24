mod content;
mod doc_extract;
mod file;
mod file_index;
mod fragment;
mod rg;
mod sanitize;
mod session;

pub use content::ContentDlp;
pub use file::FileDlp;
pub use session::SessionGuard;

use crate::config::AppConfig;
use smr_protocol::{extract_tool_call_texts, ExtractedText, TextPointer};

pub struct DlpEngine {
    content: ContentDlp,
    file: FileDlp,
    sessions: SessionGuard,
    enabled: bool,
}

impl DlpEngine {
    pub fn new(config: &AppConfig) -> anyhow::Result<Self> {
        Self::with_sessions(config, SessionGuard::new())
    }

    pub fn with_sessions(config: &AppConfig, sessions: SessionGuard) -> anyhow::Result<Self> {
        let enabled = config.pipeline.dlp_active();
        Ok(Self {
            content: ContentDlp::new(&config.content_rules, &config.pipeline)?,
            file: FileDlp::new(&config.file_rules)?,
            sessions,
            enabled,
        })
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

    pub fn process_request(
        &self,
        session_id: &str,
        extracted: &[ExtractedText],
        request_json: &serde_json::Value,
    ) -> anyhow::Result<(Vec<(ExtractedText, String)>, usize)> {
        if !self.enabled {
            return Ok((Vec::new(), 0));
        }

        self.apply_path_triggers(session_id, request_json);

        let session_active = self.sessions.begin_request(session_id);
        let mut replacements = Vec::new();
        for item in extracted {
            let sanitized = self.sanitize_field(&item.text, session_active.as_deref())?;
            if sanitized != item.text {
                replacements.push((item.clone(), sanitized));
            }
        }
        let count = replacements.len();
        Ok((replacements, count))
    }

    /// Response-side: path triggers, then sanitize fields in the same response.
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
            let sanitized = self.sanitize_field(&item.text, session_active.as_deref())?;
            if sanitized != item.text {
                replacements.push((item.clone(), sanitized));
            }
        }
        let count = replacements.len();
        Ok((replacements, count))
    }

    fn apply_path_triggers(&self, session_id: &str, body: &serde_json::Value) {
        let Ok(tool_texts) = extract_tool_call_texts(body) else {
            return;
        };
        let tool_blob: String = tool_texts
            .iter()
            .map(|t| t.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if tool_blob.is_empty() {
            return;
        }
        self.file
            .check_path_triggers_in_tool_text(session_id, &tool_blob, |sid, rule, contents| {
                self.sessions.activate(sid, rule, contents, rule.trigger_window);
            });
    }

    fn sanitize_field(
        &self,
        text: &str,
        session_active: Option<&[session::ActiveFileContent]>,
    ) -> anyhow::Result<String> {
        let sanitized = self.content.sanitize_text(text)?;
        if let Some(active) = session_active {
            Ok(self.sessions.sanitize_with_active(&sanitized, active, &self.file))
        } else {
            Ok(sanitized)
        }
    }

    pub fn is_tool_field(pointer: &TextPointer) -> bool {
        matches!(pointer, TextPointer::OpenAiToolCallArguments { .. })
    }
}
