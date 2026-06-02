//! In-memory ring buffer of request/response bodies for debugging (optional).

use std::collections::VecDeque;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct TrafficRecord {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub session_id: String,
    pub audit_id: String,
    pub phase: String,
    pub bytes: usize,
    pub body: String,
}

pub struct TrafficLog {
    inner: Mutex<VecDeque<TrafficRecord>>,
    max_entries: usize,
}

impl TrafficLog {
    pub fn new(max_entries: usize) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(VecDeque::new()),
            max_entries: max_entries.max(10),
        })
    }

    pub fn record(
        &self,
        audit_id: &str,
        session_id: &str,
        phase: &str,
        body: &[u8],
        max_bytes: usize,
    ) {
        let cap = max_bytes.max(1024).min(512 * 1024);
        let truncated = body.len() > cap;
        let slice = &body[..body.len().min(cap)];
        let mut text = String::from_utf8_lossy(slice).into_owned();
        if truncated {
            text.push_str("\n… (truncated)");
        }
        let entry = TrafficRecord {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            session_id: session_id.to_string(),
            audit_id: audit_id.to_string(),
            phase: phase.to_string(),
            bytes: body.len(),
            body: text,
        };
        let mut guard = self.inner.lock();
        guard.push_front(entry);
        while guard.len() > self.max_entries {
            guard.pop_back();
        }
    }

    pub fn list(&self, limit: usize) -> Vec<TrafficRecord> {
        let guard = self.inner.lock();
        guard.iter().take(limit).cloned().collect()
    }
}
