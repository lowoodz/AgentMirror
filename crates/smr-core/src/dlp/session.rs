use std::sync::Mutex;

use dashmap::DashMap;

use crate::config::FileRule;
use crate::dlp::file::FileDlp;
use crate::dlp::file::FileContent;

#[derive(Clone)]
pub struct ActiveFileContent {
    pub rule: FileRule,
    pub contents: Vec<FileContent>,
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

    pub fn activate(&self, session_id: &str, rule: &FileRule, contents: &[FileContent], window: u32) {
        let entry = self.sessions.entry(session_id.to_string()).or_insert_with(|| {
            Mutex::new(SessionState {
                active: Vec::new(),
                remaining_calls: 0,
            })
        });
        let mut state = entry.lock().unwrap();

        let already = state
            .active
            .iter()
            .any(|a| a.rule.id == rule.id && a.rule.path == rule.path);
        if !already {
            state.active.push(ActiveFileContent {
                rule: rule.clone(),
                contents: contents.to_vec(),
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

    pub fn sanitize_with_active(
        &self,
        text: &str,
        active: &[ActiveFileContent],
        file_dlp: &FileDlp,
    ) -> String {
        file_dlp.scan_text(text, active)
    }
}

impl Default for SessionGuard {
    fn default() -> Self {
        Self::new()
    }
}
