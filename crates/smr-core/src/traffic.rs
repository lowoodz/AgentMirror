//! Request/response body snapshots for debugging (optional).
//! Full bodies are written to disk; the in-memory ring buffer keeps metadata + preview.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use chrono::{DateTime, Local};
use futures::Stream;
use parking_lot::Mutex;
use serde::Serialize;

/// Hard upper bound for saved traffic body files (20 MiB).
pub const ABS_MAX_BODY_BYTES: usize = 20 * 1024 * 1024;
/// Bytes kept in memory for the admin UI preview.
const PREVIEW_BYTES: usize = 8192;

#[derive(Clone, Debug, Serialize)]
pub struct TrafficRecord {
    pub id: String,
    pub timestamp: DateTime<Local>,
    pub session_id: String,
    pub audit_id: String,
    pub phase: String,
    /// Original body length before any truncation.
    pub bytes: usize,
    /// Bytes actually written to disk.
    pub saved_bytes: usize,
    pub truncated: bool,
    /// Absolute path of the saved body file.
    pub file_path: String,
    /// Short preview for the list UI (full body is on disk).
    pub preview: String,
}

pub struct TrafficLog {
    inner: Mutex<VecDeque<TrafficRecord>>,
    max_entries: usize,
    traffic_dir: PathBuf,
}

impl TrafficLog {
    pub fn new(max_entries: usize, traffic_dir: PathBuf) -> Arc<Self> {
        let _ = std::fs::create_dir_all(&traffic_dir);
        Arc::new(Self {
            inner: Mutex::new(VecDeque::new()),
            max_entries: max_entries.max(10),
            traffic_dir,
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
        let cap = clamp_body_limit(max_bytes);
        let truncated = body.len() > cap;
        let saved = &body[..body.len().min(cap)];

        let id = uuid::Uuid::new_v4().to_string();
        let ts = Local::now();
        let file_name = format!(
            "{}_{}_{}.body",
            ts.format("%Y%m%dT%H%M%S"),
            sanitize_phase(phase),
            &id[..8]
        );
        let file_path = self.traffic_dir.join(&file_name);
        if let Err(err) = write_body_file(&file_path, saved) {
            tracing::warn!(?err, ?file_path, "failed to write traffic snapshot file");
            return;
        }

        let preview_slice = &saved[..saved.len().min(PREVIEW_BYTES)];
        let mut preview = String::from_utf8_lossy(preview_slice).into_owned();
        if saved.len() > PREVIEW_BYTES || truncated {
            preview.push_str("\n… (preview; open full body via link)");
        }

        let entry = TrafficRecord {
            id,
            timestamp: ts,
            session_id: session_id.to_string(),
            audit_id: audit_id.to_string(),
            phase: phase.to_string(),
            bytes: body.len(),
            saved_bytes: saved.len(),
            truncated,
            file_path: file_path.display().to_string(),
            preview,
        };

        let mut guard = self.inner.lock();
        guard.push_front(entry);
        while guard.len() > self.max_entries {
            if let Some(old) = guard.pop_back() {
                let _ = std::fs::remove_file(&old.file_path);
            }
        }
    }

    pub fn list(&self, limit: usize) -> Vec<TrafficRecord> {
        let guard = self.inner.lock();
        guard.iter().take(limit).cloned().collect()
    }

    pub fn read_body(&self, id: &str) -> Option<(TrafficRecord, Vec<u8>)> {
        if !is_uuid(id) {
            return None;
        }
        let guard = self.inner.lock();
        let record = guard.iter().find(|r| r.id == id)?.clone();
        drop(guard);
        let data = std::fs::read(&record.file_path).ok()?;
        Some((record, data))
    }

    pub fn traffic_dir(&self) -> &Path {
        &self.traffic_dir
    }

    /// Wrap an SSE byte stream and persist the aggregated body when the stream ends.
    pub fn wrap_sse_stream(
        self: &Arc<Self>,
        stream: Pin<Box<dyn Stream<Item = Result<Bytes, Infallible>> + Send>>,
        audit_id: &str,
        session_id: &str,
        phase: &str,
        max_bytes: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Bytes, Infallible>> + Send>> {
        let state = Arc::new(TrafficTapState {
            collector: Mutex::new(Vec::new()),
            traffic: Arc::clone(self),
            audit_id: audit_id.to_string(),
            session_id: session_id.to_string(),
            phase: phase.to_string(),
            max_bytes: clamp_body_limit(max_bytes),
            recorded: AtomicBool::new(false),
        });
        Box::pin(TrafficTapStream { inner: stream, state })
    }
}

struct TrafficTapState {
    collector: Mutex<Vec<u8>>,
    traffic: Arc<TrafficLog>,
    audit_id: String,
    session_id: String,
    phase: String,
    max_bytes: usize,
    recorded: AtomicBool,
}

impl TrafficTapState {
    fn push(&self, chunk: &[u8]) {
        let mut buf = self.collector.lock();
        if buf.len() < self.max_bytes {
            let take = (self.max_bytes - buf.len()).min(chunk.len());
            buf.extend_from_slice(&chunk[..take]);
        }
    }

    fn flush(&self) {
        if self.recorded.swap(true, Ordering::SeqCst) {
            return;
        }
        let buf = self.collector.lock().clone();
        if !buf.is_empty() {
            self.traffic.record(
                &self.audit_id,
                &self.session_id,
                &self.phase,
                &buf,
                self.max_bytes,
            );
        }
    }
}

struct TrafficTapStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, Infallible>> + Send>>,
    state: Arc<TrafficTapState>,
}

impl Stream for TrafficTapStream {
    type Item = Result<Bytes, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = unsafe { self.get_unchecked_mut() };
        match this.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(bytes))) => {
                this.state.push(&bytes);
                Poll::Ready(Some(Ok(bytes)))
            }
            Poll::Ready(None) => {
                this.state.flush();
                Poll::Ready(None)
            }
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(e))),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for TrafficTapStream {
    fn drop(&mut self) {
        self.state.flush();
    }
}

pub fn clamp_body_limit(max_bytes: usize) -> usize {
    max_bytes.max(1024).min(ABS_MAX_BODY_BYTES)
}

fn write_body_file(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(data)?;
    file.flush()?;
    Ok(())
}

fn sanitize_phase(phase: &str) -> String {
    phase
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn is_uuid(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_full_body_to_file_up_to_limit() {
        let dir = std::env::temp_dir().join(format!("smr-traffic-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = TrafficLog::new(5, dir.clone());
        let body = vec![b'x'; 100_000];
        log.record("audit", "sess", "request_in", &body, ABS_MAX_BODY_BYTES);
        let records = log.list(1);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].bytes, 100_000);
        assert_eq!(records[0].saved_bytes, 100_000);
        assert!(!records[0].truncated);
        let on_disk = std::fs::read(&records[0].file_path).unwrap();
        assert_eq!(on_disk.len(), 100_000);
        let (rec, data) = log.read_body(&records[0].id).unwrap();
        assert_eq!(rec.id, records[0].id);
        assert_eq!(data.len(), 100_000);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn truncates_beyond_configured_max() {
        let dir = std::env::temp_dir().join(format!("smr-traffic-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = TrafficLog::new(5, dir.clone());
        let body = vec![b'y'; 5000];
        log.record("audit", "sess", "response_out", &body, 2000);
        let records = log.list(1);
        assert!(records[0].truncated);
        assert_eq!(records[0].saved_bytes, 2000);
        let on_disk = std::fs::read(&records[0].file_path).unwrap();
        assert_eq!(on_disk.len(), 2000);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn clamp_never_exceeds_abs_max() {
        assert_eq!(clamp_body_limit(usize::MAX), ABS_MAX_BODY_BYTES);
        assert_eq!(clamp_body_limit(0), 1024);
    }
}
