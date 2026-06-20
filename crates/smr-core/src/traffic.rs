//! Request/response body snapshots for debugging (optional).
//! Full bodies are written to disk; metadata is kept in memory and sidecar JSON files.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use chrono::{DateTime, Duration, Local, NaiveDateTime};
use futures::Stream;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::config::LoggingConfig;

/// Hard upper bound for saved traffic body files (20 MiB).
pub const ABS_MAX_BODY_BYTES: usize = 20 * 1024 * 1024;
/// Default retention window for traffic snapshots.
pub const DEFAULT_RETENTION_DAYS: u32 = 7;
/// Default total on-disk cap for all traffic snapshot files (1 GiB).
pub const DEFAULT_MAX_DISK_BYTES: u64 = 1024 * 1024 * 1024;
/// Minimum configured total disk cap.
pub const MIN_MAX_DISK_BYTES: u64 = 1024 * 1024;
/// Bytes kept in memory for the admin UI preview.
const PREVIEW_BYTES: usize = 8192;

#[derive(Clone, Debug, Serialize, Deserialize)]
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
    #[serde(default)]
    pub preview: String,
}

pub struct TrafficLog {
    inner: Mutex<VecDeque<TrafficRecord>>,
    traffic_dir: PathBuf,
    retention_days: Mutex<u32>,
    max_disk_bytes: Mutex<u64>,
}

impl TrafficLog {
    pub fn from_logging_config(logging: &LoggingConfig, traffic_dir: PathBuf) -> Arc<Self> {
        let _ = std::fs::create_dir_all(&traffic_dir);
        let log = Arc::new(Self {
            inner: Mutex::new(VecDeque::new()),
            traffic_dir,
            retention_days: Mutex::new(logging.traffic_retention_days.max(1)),
            max_disk_bytes: Mutex::new(logging.traffic_max_disk_bytes),
        });
        log.reload_from_disk();
        log.enforce_retention();
        log
    }

    pub fn apply_policy(&self, logging: &LoggingConfig) {
        *self.retention_days.lock() = logging.traffic_retention_days.max(1);
        *self.max_disk_bytes.lock() = logging.traffic_max_disk_bytes;
        self.enforce_retention();
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

        let preview = preview_for_body(saved, truncated);

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

        if let Err(err) = write_meta_file(&meta_path_for_body(&file_path), &entry) {
            tracing::warn!(?err, "failed to write traffic snapshot metadata");
            let _ = std::fs::remove_file(&file_path);
            return;
        }

        self.inner.lock().push_front(entry);
        self.enforce_retention();
    }

    pub fn list(&self, limit: usize) -> Vec<TrafficRecord> {
        let guard = self.inner.lock();
        guard.iter().take(limit).cloned().collect()
    }

    pub fn list_by_audit(&self, audit_id: &str) -> Vec<TrafficRecord> {
        let guard = self.inner.lock();
        guard
            .iter()
            .filter(|r| r.audit_id == audit_id)
            .cloned()
            .collect()
    }

    /// Load request/response bodies saved for an audit (`request_out` + `response_out`).
    /// When `response_out` is client-facing SSE without usage, fall back to upstream `response_in`.
    pub fn bodies_for_audit(&self, audit_id: &str) -> Option<(Vec<u8>, Vec<u8>)> {
        let records = self.list_by_audit(audit_id);
        if records.is_empty() {
            return None;
        }
        let request = read_body_for_phases(&records, &["request_out", "request_in"]);
        let response = response_body_for_replay(&records);
        if request.is_none() && response.is_none() {
            return None;
        }
        Some((request.unwrap_or_default(), response.unwrap_or_default()))
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

    fn reload_from_disk(&self) {
        let mut records = scan_traffic_dir(&self.traffic_dir);
        records.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        *self.inner.lock() = records.into_iter().collect();
    }

    fn enforce_retention(&self) {
        let retention_days = *self.retention_days.lock();
        let max_disk = *self.max_disk_bytes.lock();
        let cutoff = Local::now() - Duration::days(retention_days as i64);

        let mut guard = self.inner.lock();
        let expired: Vec<TrafficRecord> = guard
            .iter()
            .filter(|r| r.timestamp < cutoff)
            .cloned()
            .collect();
        for record in expired {
            delete_record_files(&record);
        }
        guard.retain(|r| r.timestamp >= cutoff);

        let mut ordered: Vec<TrafficRecord> = guard.iter().cloned().collect();
        ordered.sort_by(|a, b| {
            a.timestamp
                .cmp(&b.timestamp)
                .then_with(|| a.file_path.cmp(&b.file_path))
        });
        let mut total = ordered
            .iter()
            .map(|r| body_file_size(&r.file_path))
            .sum::<u64>();

        while total > max_disk && !ordered.is_empty() {
            let old = ordered.remove(0);
            total = total.saturating_sub(body_file_size(&old.file_path));
            delete_record_files(&old);
            guard.retain(|r| r.id != old.id);
        }
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

fn meta_path_for_body(body_path: &Path) -> PathBuf {
    body_path.with_extension("meta.json")
}

fn write_meta_file(meta_path: &Path, record: &TrafficRecord) -> std::io::Result<()> {
    let stored = StoredTrafficRecord {
        id: record.id.clone(),
        timestamp: record.timestamp,
        session_id: record.session_id.clone(),
        audit_id: record.audit_id.clone(),
        phase: record.phase.clone(),
        bytes: record.bytes,
        saved_bytes: record.saved_bytes,
        truncated: record.truncated,
        file_path: record.file_path.clone(),
    };
    let json = serde_json::to_string_pretty(&stored)?;
    std::fs::write(meta_path, json)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredTrafficRecord {
    id: String,
    timestamp: DateTime<Local>,
    session_id: String,
    audit_id: String,
    phase: String,
    bytes: usize,
    saved_bytes: usize,
    truncated: bool,
    file_path: String,
}

fn scan_traffic_dir(traffic_dir: &Path) -> Vec<TrafficRecord> {
    let mut records = Vec::new();
    let Ok(entries) = std::fs::read_dir(traffic_dir) else {
        return records;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !name.ends_with(".meta.json") {
            continue;
        }
        if let Some(record) = load_record_from_meta(&path) {
            records.push(record);
        }
    }

    let Ok(entries) = std::fs::read_dir(traffic_dir) else {
        return records;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("body") {
            continue;
        }
        let meta_path = meta_path_for_body(&path);
        if meta_path.exists() {
            continue;
        }
        if let Some(record) = load_legacy_body_record(&path) {
            if write_meta_file(&meta_path, &record).is_ok() {
                records.push(record);
            }
        }
    }

    records
}

fn load_record_from_meta(meta_path: &Path) -> Option<TrafficRecord> {
    let text = std::fs::read_to_string(meta_path).ok()?;
    let stored: StoredTrafficRecord = serde_json::from_str(&text).ok()?;
    let body_path = PathBuf::from(&stored.file_path);
    if !body_path.exists() {
        let _ = std::fs::remove_file(meta_path);
        return None;
    }
    let on_disk = std::fs::read(&body_path).ok()?;
    Some(TrafficRecord {
        id: stored.id,
        timestamp: stored.timestamp,
        session_id: stored.session_id,
        audit_id: stored.audit_id,
        phase: stored.phase,
        bytes: stored.bytes,
        saved_bytes: stored.saved_bytes,
        truncated: stored.truncated,
        file_path: stored.file_path,
        preview: preview_for_body(&on_disk, stored.truncated),
    })
}

fn load_legacy_body_record(body_path: &Path) -> Option<TrafficRecord> {
    let on_disk = std::fs::read(body_path).ok()?;
    let stem = body_path.file_stem()?.to_str()?;
    let (timestamp, phase) = parse_legacy_filename(stem)?;
    let id = legacy_id_for_path(body_path);
    Some(TrafficRecord {
        id,
        timestamp,
        session_id: String::new(),
        audit_id: String::new(),
        phase,
        bytes: on_disk.len(),
        saved_bytes: on_disk.len(),
        truncated: false,
        file_path: body_path.display().to_string(),
        preview: preview_for_body(&on_disk, false),
    })
}

fn parse_legacy_filename(stem: &str) -> Option<(DateTime<Local>, String)> {
    if stem.len() < 15 + 1 + 8 + 1 || stem.as_bytes().get(15)? != &b'_' {
        return None;
    }
    let ts_str = &stem[..15];
    let naive = NaiveDateTime::parse_from_str(ts_str, "%Y%m%dT%H%M%S").ok()?;
    let timestamp = DateTime::<Local>::from_naive_utc_and_offset(naive, *Local::now().offset());
    let phase = stem[16..stem.len().saturating_sub(9)].to_string();
    if phase.is_empty() {
        return None;
    }
    Some((timestamp, phase))
}

fn legacy_id_for_path(path: &Path) -> String {
    uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_URL,
        path.to_string_lossy().as_bytes(),
    )
    .to_string()
}

fn preview_for_body(saved: &[u8], truncated: bool) -> String {
    let preview_slice = &saved[..saved.len().min(PREVIEW_BYTES)];
    let mut preview = String::from_utf8_lossy(preview_slice).into_owned();
    if saved.len() > PREVIEW_BYTES || truncated {
        preview.push_str("\n… (preview; open full body via link)");
    }
    preview
}

fn delete_record_files(record: &TrafficRecord) {
    let body_path = PathBuf::from(&record.file_path);
    let _ = std::fs::remove_file(&body_path);
    let _ = std::fs::remove_file(meta_path_for_body(&body_path));
}

fn body_file_size(path: &str) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
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

fn read_body_for_phases(records: &[TrafficRecord], phases: &[&str]) -> Option<Vec<u8>> {
    for phase in phases {
        if let Some(record) = records.iter().find(|r| r.phase == *phase) {
            if let Ok(data) = std::fs::read(&record.file_path) {
                if !data.is_empty() {
                    return Some(data);
                }
            }
        }
    }
    None
}

fn response_body_for_replay(records: &[TrafficRecord]) -> Option<Vec<u8>> {
    let out = read_body_for_phases(records, &["response_out"]);
    let upstream = read_body_for_phases(records, &["response_in"]);
    match (out, upstream) {
        (Some(client), Some(up)) => {
            if smr_insight::usage::extract_token_usage(&client).is_empty()
                && !smr_insight::usage::extract_token_usage(&up).is_empty()
            {
                Some(up)
            } else {
                Some(client)
            }
        }
        (Some(client), None) => Some(client),
        (None, Some(up)) => Some(up),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LoggingConfig;

    fn test_log(dir: PathBuf) -> Arc<TrafficLog> {
        let mut logging = LoggingConfig::default();
        logging.traffic_retention_days = 7;
        logging.traffic_max_disk_bytes = DEFAULT_MAX_DISK_BYTES;
        TrafficLog::from_logging_config(&logging, dir)
    }

    #[test]
    fn saves_full_body_to_file_up_to_limit() {
        let dir = std::env::temp_dir().join(format!("smr-traffic-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = test_log(dir.clone());
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
        assert!(meta_path_for_body(Path::new(&records[0].file_path)).exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn truncates_beyond_configured_max() {
        let dir = std::env::temp_dir().join(format!("smr-traffic-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = test_log(dir.clone());
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

    #[test]
    fn reloads_records_from_disk_after_restart() {
        let dir = std::env::temp_dir().join(format!("smr-traffic-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = test_log(dir.clone());
        log.record("audit", "sess-1", "request_in", b"hello", ABS_MAX_BODY_BYTES);
        let id = log.list(1)[0].id.clone();

        let log2 = test_log(dir.clone());
        let records = log2.list(10);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, id);
        assert_eq!(records[0].session_id, "sess-1");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn bodies_for_audit_prefers_response_in_when_out_has_no_usage() {
        let dir = std::env::temp_dir().join(format!("smr-traffic-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = test_log(dir.clone());
        let audit = "audit-usage-fallback";
        let sse = b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n";
        let json = br#"{"choices":[{"message":{"content":"ok"}}],"usage":{"prompt_tokens":9,"completion_tokens":3,"total_tokens":12}}"#;
        log.record(audit, "sess", "response_out", sse, ABS_MAX_BODY_BYTES);
        log.record(audit, "sess", "response_in", json, ABS_MAX_BODY_BYTES);
        log.record(audit, "sess", "request_out", br#"{"messages":[]}"#, ABS_MAX_BODY_BYTES);

        let (req, resp) = log.bodies_for_audit(audit).unwrap();
        assert_eq!(req, br#"{"messages":[]}"#);
        assert_eq!(resp, json);
        let usage = smr_insight::usage::extract_token_usage(&resp);
        assert_eq!(usage.total_tokens, 12);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn enforces_total_disk_cap() {
        let dir = std::env::temp_dir().join(format!("smr-traffic-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut logging = LoggingConfig::default();
        logging.traffic_retention_days = 7;
        logging.traffic_max_disk_bytes = 5000;
        let log = TrafficLog::from_logging_config(&logging, dir.clone());

        log.record("a1", "s1", "request_in", &vec![b'a'; 2000], ABS_MAX_BODY_BYTES);
        std::thread::sleep(std::time::Duration::from_millis(1100));
        log.record("a2", "s2", "request_in", &vec![b'b'; 2000], ABS_MAX_BODY_BYTES);
        std::thread::sleep(std::time::Duration::from_millis(1100));
        log.record("a3", "s3", "request_in", &vec![b'c'; 2000], ABS_MAX_BODY_BYTES);

        let records = log.list(10);
        assert_eq!(records.len(), 2);
        assert!(records.iter().any(|r| r.session_id == "s2"));
        assert!(records.iter().any(|r| r.session_id == "s3"));
        assert!(!records.iter().any(|r| r.session_id == "s1"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
