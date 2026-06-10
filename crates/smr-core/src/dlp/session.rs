use std::sync::Mutex;

use dashmap::DashMap;

use crate::config::FileRule;
use crate::dlp::file::FileDlp;

#[derive(Clone)]
pub struct ActiveFileContent {
    pub rule: FileRule,
    /// Normalized paths of files mentioned in tool calls (scan scope).
    pub triggered_files: Vec<String>,
}

struct SessionState {
    active: Vec<ActiveFileContent>,
    remaining_calls: u32,
}

pub struct SessionGuard {
    sessions: DashMap<String, Mutex<SessionState>>,
}

impl Clone for SessionGuard {
    fn clone(&self) -> Self {
        let cloned = Self::new();
        for entry in self.sessions.iter() {
            let state = entry.value().lock().unwrap();
            cloned.sessions.insert(
                entry.key().clone(),
                Mutex::new(SessionState {
                    active: state.active.clone(),
                    remaining_calls: state.remaining_calls,
                }),
            );
        }
        cloned
    }
}

impl SessionGuard {
    pub fn new() -> Self {
        Self {
            sessions: DashMap::new(),
        }
    }

    pub fn activate(
        &self,
        session_id: &str,
        rule: &FileRule,
        triggered_files: &[String],
        window: u32,
    ) {
        if triggered_files.is_empty() {
            return;
        }
        let entry = self.sessions.entry(session_id.to_string()).or_insert_with(|| {
            Mutex::new(SessionState {
                active: Vec::new(),
                remaining_calls: 0,
            })
        });
        let mut state = entry.lock().unwrap();

        if let Some(existing) = state
            .active
            .iter_mut()
            .find(|a| a.rule.id == rule.id && a.rule.path == rule.path)
        {
            for path in triggered_files {
                if !existing.triggered_files.iter().any(|p| p == path) {
                    existing.triggered_files.push(path.clone());
                }
            }
        } else {
            state.active.push(ActiveFileContent {
                rule: rule.clone(),
                triggered_files: triggered_files.to_vec(),
            });
        }
        state.remaining_calls = state.remaining_calls.max(window);
    }

    /// Consume one model-call slot and return active file rules for this request.
    pub fn begin_request(&self, session_id: &str) -> Option<Vec<ActiveFileContent>> {
        let key = session_id.to_string();
        let entry = self.sessions.get(&key)?;
        let mut state = entry.lock().unwrap();
        if state.remaining_calls == 0 || state.active.is_empty() {
            return None;
        }
        state.remaining_calls -= 1;
        Some(state.active.clone())
    }

    /// Active rules without consuming a call slot (same HTTP response turn).
    pub fn active_snapshot(&self, session_id: &str) -> Option<Vec<ActiveFileContent>> {
        let key = session_id.to_string();
        let entry = self.sessions.get(&key)?;
        let state = entry.lock().unwrap();
        if state.remaining_calls == 0 || state.active.is_empty() {
            return None;
        }
        Some(state.active.clone())
    }

    /// Restore `remaining_calls` to each active rule's `trigger_window` (after ops rewrites tool paths).
    pub fn reboost_windows(&self, session_id: &str) {
        let Some(entry) = self.sessions.get(session_id) else {
            return;
        };
        let mut state = entry.lock().unwrap();
        if let Some(max_window) = state
            .active
            .iter()
            .map(|active| active.rule.trigger_window)
            .max()
        {
            state.remaining_calls = state.remaining_calls.max(max_window);
        }
    }

    pub fn sanitize_with_active(
        &self,
        text: &str,
        active: &[ActiveFileContent],
        file_dlp: &FileDlp,
        vault: Option<(&str, &crate::dlp::TokenVault)>,
        whole_block_on_match: bool,
        tool_output_block_message: &str,
    ) -> String {
        file_dlp.scan_text(
            text,
            active,
            vault,
            whole_block_on_match,
            tool_output_block_message,
        )
    }
}

impl Default for SessionGuard {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::config::{FileRule, MatchMode};

    use super::*;

    fn test_rule(id: &str, window: u32) -> FileRule {
        FileRule {
            id: id.into(),
            enabled: true,
            path: PathBuf::from("/tmp/secrets"),
            recursive: true,
            trigger_window: window,
            match_mode: MatchMode::Full,
            min_fragment_len: None,
            min_fragment_ratio: None,
            formats: vec!["txt".into()],
            index: Default::default(),
        }
    }

    #[test]
    fn reboost_restores_trigger_window_after_begin_request() {
        let guard = SessionGuard::new();
        let rule = test_rule("r1", 2);
        guard.activate("sess", &rule, &["/tmp/secrets/a.txt".into()], 2);
        assert_eq!(guard.begin_request("sess").map(|v| v.len()), Some(1));
        guard.reboost_windows("sess");
        assert!(guard.begin_request("sess").is_some());
        assert!(guard.begin_request("sess").is_some());
        assert!(guard.begin_request("sess").is_none());
    }
}
