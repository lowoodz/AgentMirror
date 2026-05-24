mod content;
mod file;
mod rg;
mod sanitize;
mod session;

pub use content::ContentDlp;
pub use file::FileDlp;
pub use session::SessionGuard;

use crate::config::AppConfig;
use smr_protocol::ExtractedText;

pub struct DlpEngine {
    content: ContentDlp,
    file: FileDlp,
    sessions: SessionGuard,
    enabled: bool,
}

impl DlpEngine {
    pub fn new(config: &AppConfig) -> anyhow::Result<Self> {
        Ok(Self {
            content: ContentDlp::new(&config.content_rules)?,
            file: FileDlp::new(&config.file_rules)?,
            sessions: SessionGuard::new(),
            enabled: config.pipeline.dlp_enabled,
        })
    }

    pub fn process_request(
        &self,
        session_id: &str,
        all_text: &str,
        extracted: &[ExtractedText],
    ) -> anyhow::Result<Vec<(ExtractedText, String)>> {
        if !self.enabled {
            return Ok(Vec::new());
        }

        self.file.check_path_triggers(session_id, all_text, |sid, rule, contents| {
            self.sessions.activate(sid, rule, contents, rule.trigger_window);
        });

        let mut replacements = Vec::new();
        for item in extracted {
            let sanitized = self.content.sanitize_text(&item.text)?;
            let sanitized = self.sessions.sanitize_with_session(session_id, &sanitized)?;
            if sanitized != item.text {
                replacements.push((item.clone(), sanitized));
            }
        }
        Ok(replacements)
    }
}
