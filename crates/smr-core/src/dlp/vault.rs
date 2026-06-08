use std::collections::HashMap;
use std::sync::Mutex;

use dashmap::DashMap;

use crate::dlp::sanitize::sanitize_whole;

/// Per-session reversible redaction tokens. Plaintext never leaves the process; only
/// `[[smr:xxxxxxxx]]` placeholders are sent to upstream models.
#[derive(Debug, Default)]
pub struct TokenVault {
    sessions: DashMap<String, Mutex<SessionVault>>,
}

const MAX_SECRETS_PER_SESSION: usize = 4096;

#[derive(Debug, Default)]
struct SessionVault {
    by_plaintext: HashMap<String, String>,
    by_token: HashMap<String, String>,
    next_id: u32,
}

impl Clone for TokenVault {
    fn clone(&self) -> Self {
        let cloned = Self::default();
        for entry in self.sessions.iter() {
            let state = entry.value().lock().unwrap();
            cloned.sessions.insert(
                entry.key().clone(),
                Mutex::new(SessionVault {
                    by_plaintext: state.by_plaintext.clone(),
                    by_token: state.by_token.clone(),
                    next_id: state.next_id,
                }),
            );
        }
        cloned
    }
}

impl TokenVault {
    pub fn new() -> Self {
        Self::default()
    }

    /// Return a stable placeholder for `plaintext` within `session_id`.
    pub fn token_for(&self, session_id: &str, plaintext: &str) -> String {
        if plaintext.is_empty() {
            return String::new();
        }
        let entry = self
            .sessions
            .entry(session_id.to_string())
            .or_insert_with(|| Mutex::new(SessionVault::default()));
        let mut state = entry.lock().unwrap();
        if let Some(token) = state.by_plaintext.get(plaintext) {
            return token.clone();
        }
        if state.by_plaintext.len() >= MAX_SECRETS_PER_SESSION {
            return sanitize_whole(plaintext);
        }
        state.next_id += 1;
        let token = format!("[[smr:{:08x}]]", state.next_id);
        state
            .by_plaintext
            .insert(plaintext.to_string(), token.clone());
        state
            .by_token
            .insert(token.clone(), plaintext.to_string());
        token
    }

    /// Replace known tokens in `text` with their original plaintext (tool-call restore path).
    pub fn restore(&self, session_id: &str, text: &str) -> String {
        let Some(entry) = self.sessions.get(session_id) else {
            return text.to_string();
        };
        let state = entry.lock().unwrap();
        if state.by_token.is_empty() {
            return text.to_string();
        }
        let mut tokens: Vec<&String> = state.by_token.keys().collect();
        tokens.sort_by_key(|t| std::cmp::Reverse(t.len()));
        let mut result = text.to_string();
        for token in tokens {
            if let Some(plain) = state.by_token.get(token) {
                if result.contains(token.as_str()) {
                    result = result.replace(token.as_str(), plain);
                }
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_token_per_plaintext() {
        let vault = TokenVault::new();
        let t1 = vault.token_for("sess", "my-password");
        let t2 = vault.token_for("sess", "my-password");
        assert_eq!(t1, t2);
        assert!(t1.starts_with("[[smr:"));
    }

    #[test]
    fn restore_round_trip() {
        let vault = TokenVault::new();
        let token = vault.token_for("sess", "ssh-secret-pass");
        let redacted = format!(r#"{{"password":"{token}"}}"#);
        let restored = vault.restore("sess", &redacted);
        assert!(restored.contains("ssh-secret-pass"));
        assert!(!restored.contains("[[smr:"));
    }

    #[test]
    fn sessions_isolated() {
        let vault = TokenVault::new();
        let t = vault.token_for("a", "secret");
        assert_eq!(vault.restore("b", &t), t);
        assert_eq!(vault.restore("a", &t), "secret");
    }
}
