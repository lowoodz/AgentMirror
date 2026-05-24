use std::collections::VecDeque;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct EventRecord {
    pub timestamp: DateTime<Utc>,
    pub kind: EventKind,
    pub message: String,
    pub rule_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    DlpReplace,
    OpBlock,
    OpObserve,
    RouteFallback,
    RouteSuccess,
    ConfigReload,
    Error,
    Info,
}

pub struct EventLog {
    max: usize,
    entries: RwLock<VecDeque<EventRecord>>,
}

impl EventLog {
    pub fn new(max: usize) -> Arc<Self> {
        Arc::new(Self {
            max,
            entries: RwLock::new(VecDeque::with_capacity(max.min(256))),
        })
    }

    pub fn push(&self, kind: EventKind, message: impl Into<String>, rule_id: Option<String>) {
        let record = EventRecord {
            timestamp: Utc::now(),
            kind,
            message: message.into(),
            rule_id,
        };
        let mut q = self.entries.write();
        if q.len() >= self.max {
            q.pop_front();
        }
        q.push_back(record);
    }

    pub fn list(&self, limit: usize) -> Vec<EventRecord> {
        let q = self.entries.read();
        q.iter().rev().take(limit).cloned().collect()
    }
}

impl Default for EventLog {
    fn default() -> Self {
        Self {
            max: 500,
            entries: RwLock::new(VecDeque::new()),
        }
    }
}
