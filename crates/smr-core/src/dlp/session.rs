use std::sync::Mutex;

use dashmap::DashMap;

use crate::config::FileRule;
use crate::dlp::file::{scan_text_for_file_content, FileContent};

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

        state.active.push(ActiveFileContent {
            rule: rule.clone(),
            contents: contents.to_vec(),
        });
        state.remaining_calls = state.remaining_calls.max(window);
    }

    pub fn sanitize_with_session(&self, session_id: &str, text: &str) -> anyhow::Result<String> {
        let key = session_id.to_string();
        let Some(entry) = self.sessions.get(&key) else {
            return Ok(text.to_string());
        };

        let mut state = entry.lock().unwrap();
        if state.remaining_calls == 0 || state.active.is_empty() {
            return Ok(text.to_string());
        }

        state.remaining_calls -= 1;
        let active = state.active.clone();
        Ok(scan_text_for_file_content(text, &active))
    }
}

impl Default for SessionGuard {
    fn default() -> Self {
        Self::new()
    }
}
