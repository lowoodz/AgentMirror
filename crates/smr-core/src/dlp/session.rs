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
