//! Disk-backed file index: SQLite signatures + Bloom pre-filter (P0–P2 large corpus).

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};
use std::thread;

use anyhow::{Context, Result};
use parking_lot::RwLock;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;
use xxhash_rust::xxh64::xxh64;

use crate::config::{FileRule, MatchMode};
use crate::dlp::bloom::BloomFilter;
use crate::dlp::doc_extract;
use crate::dlp::file::{path_basename, path_trigger_match, paths_equivalent, strip_verbatim_path_prefix};
use crate::dlp::fragment::effective_min_fragment_len;
use crate::dlp::rg::find_literal_byte_offsets;
use crate::dlp::sanitize::{sanitize_range, sanitize_whole};
use crate::dlp::session::ActiveFileContent;
use crate::paths;

pub const INDEX_READ_CAP: u64 = 16 * 1024 * 1024;

const GEN_DIR: &str = "gen";
const CURRENT_FILE: &str = "current.json";
const LITERALS_FILE: &str = "literals.json";
/// Haystack length at which ripgrep literal prefilter is enabled.
const RG_PREFILTER_MIN_BYTES: usize = 8192;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FileFingerprint {
    path: String,
    mtime_secs: u64,
    size: u64,
}

#[derive(Serialize, Deserialize)]
struct FilesManifest {
    files: Vec<FileFingerprint>,
}

#[derive(Serialize, Deserialize)]
struct CurrentPointer {
    generation: u64,
}

#[derive(Serialize, Deserialize)]
struct RuleManifest {
    generation: u64,
    signatures: u64,
    files: usize,
    skipped: usize,
    reindexed: usize,
}

struct PreviousGeneration {
    generation: u64,
    db_path: PathBuf,
    files: HashMap<String, FileFingerprint>,
}

#[derive(Clone)]
pub struct IndexedRule {
    pub rule: FileRule,
    pub normalized_path: String,
}

struct RuleSnapshot {
    generation: u64,
    bloom: BloomFilter,
    db_path: PathBuf,
    literals: Arc<Vec<String>>,
    indexed_paths: Arc<HashSet<String>>,
}

struct IndexState {
    ready: AtomicBool,
    rules: Vec<IndexedRule>,
    snapshots: std::collections::HashMap<String, Arc<RuleSnapshot>>,
}

pub struct FileIndexManager {
    inner: Arc<RwLock<IndexState>>,
    index_root: PathBuf,
    alive: Arc<AtomicBool>,
}

static INDEX_BUILD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn index_build_lock() -> std::sync::MutexGuard<'static, ()> {
    INDEX_BUILD_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

struct SigRow {
    hash: u64,
    path: String,
    byte_offset: u64,
    byte_len: u32,
}

impl Drop for FileIndexManager {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
    }
}

impl FileIndexManager {
    pub fn new(rules: &[FileRule]) -> Self {
        let index_root = paths::config_dir().join("file-index");
        let alive = Arc::new(AtomicBool::new(true));
        let inner = Arc::new(RwLock::new(IndexState {
            ready: AtomicBool::new(false),
            rules: Vec::new(),
            snapshots: std::collections::HashMap::new(),
        }));
        let mgr = Self {
            inner: inner.clone(),
            index_root: index_root.clone(),
            alive: alive.clone(),
        };
        let rules_vec = rules.to_vec();
        thread::spawn(move || {
            if !alive.load(Ordering::Acquire) {
                return;
            }
            if let Ok(state) = build_all_rules(&index_root, &rules_vec) {
                if !alive.load(Ordering::Acquire) {
                    return;
                }
                let mut guard = inner.write();
                guard.ready.store(true, Ordering::Release);
                guard.rules = state.rules;
                guard.snapshots = state.snapshots;
            } else {
                tracing::error!("file index build failed");
            }
        });
        mgr.spawn_watcher(rules);
        mgr
    }

    pub fn is_ready(&self) -> bool {
        self.inner.read().ready.load(Ordering::Acquire)
    }

    pub fn rules(&self) -> Vec<IndexedRule> {
        self.inner.read().rules.clone()
    }

    pub fn rebuild_sync(&self, rules: &[FileRule]) -> Result<()> {
        self.inner.write().ready.store(false, Ordering::Release);
        let state = build_all_rules(&self.index_root, rules)?;
        let mut guard = self.inner.write();
        guard.ready.store(true, Ordering::Release);
        guard.rules = state.rules;
        guard.snapshots = state.snapshots;
        Ok(())
    }

    pub fn scan_and_sanitize(&self, text: &str, active: &[ActiveFileContent]) -> String {
        let guard = self.inner.read();
        if !guard.ready.load(Ordering::Acquire) {
            return text.to_string();
        }
        let mut result = text.to_string();
        for item in active {
            if item.triggered_files.is_empty() {
                continue;
            }
            let Some(snapshot) = guard.snapshots.get(&item.rule.id) else {
                continue;
            };
            let allowed: HashSet<String> = item.triggered_files.iter().cloned().collect();
            result = scan_haystack(&result, &item.rule, snapshot, &allowed);
        }
        result
    }

    /// Keep only tool-mentioned paths that exist in this rule's index.
    pub fn resolve_triggered_files(&self, rule_id: &str, candidates: &[String]) -> Vec<String> {
        let guard = self.inner.read();
        let Some(snapshot) = guard.snapshots.get(rule_id) else {
            return Vec::new();
        };
        let mut out = HashSet::new();
        for candidate in candidates {
            let norm = normalize_index_path(&PathBuf::from(candidate));
            if snapshot.indexed_paths.contains(&norm) {
                out.insert(norm);
                continue;
            }
            let cand_base = path_basename(&norm);
            for indexed in snapshot.indexed_paths.iter() {
                if paths_equivalent(indexed, &norm)
                    || paths_equivalent(indexed, candidate)
                    || indexed.ends_with(&format!("/{}", candidate.trim_start_matches('/')))
                    || (!cand_base.is_empty() && path_basename(indexed) == cand_base)
                {
                    out.insert(indexed.clone());
                }
            }
        }
        out.into_iter().collect()
    }

    fn spawn_watcher(&self, rules: &[FileRule]) {
        let paths: Vec<PathBuf> = rules
            .iter()
            .filter(|r| r.enabled)
            .map(|r| r.path.clone())
            .collect();
        if paths.is_empty() {
            return;
        }
        let inner = self.inner.clone();
        let index_root = self.index_root.clone();
        let rules_owned = rules.to_vec();
        let alive = self.alive.clone();
        thread::spawn(move || {
            use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
            let (tx, rx) = mpsc::channel();
            let Ok(mut watcher) = RecommendedWatcher::new(
                move |res: Result<notify::Event, notify::Error>| {
                    if let Ok(event) = res {
                        if matches!(
                            event.kind,
                            EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                        ) {
                            let _ = tx.send(());
                        }
                    }
                },
                Config::default(),
            ) else {
                return;
            };
            for p in &paths {
                let mode = if p.is_dir() {
                    RecursiveMode::Recursive
                } else {
                    RecursiveMode::NonRecursive
                };
                let _ = watcher.watch(p, mode);
            }
            while rx.recv().is_ok() {
                if !alive.load(Ordering::Acquire) {
                    break;
                }
                thread::sleep(std::time::Duration::from_millis(2000));
                if !alive.load(Ordering::Acquire) {
                    break;
                }
                if let Ok(state) = build_all_rules(&index_root, &rules_owned) {
                    if !alive.load(Ordering::Acquire) {
                        break;
                    }
                    let mut guard = inner.write();
                    guard.ready.store(true, Ordering::Release);
                    guard.rules = state.rules;
                    guard.snapshots = state.snapshots;
                }
            }
        });
    }
}

struct BuiltState {
    rules: Vec<IndexedRule>,
    snapshots: std::collections::HashMap<String, Arc<RuleSnapshot>>,
}

fn build_all_rules(index_root: &Path, rules: &[FileRule]) -> Result<BuiltState> {
    let _lock = index_build_lock();
    fs::create_dir_all(index_root)?;
    let mut indexed_rules = Vec::new();
    let mut snapshots = std::collections::HashMap::new();
    for rule in rules.iter().filter(|r| r.enabled) {
        let normalized_path = rule.path.to_string_lossy().replace('\\', "/");
        indexed_rules.push(IndexedRule {
            rule: rule.clone(),
            normalized_path: normalized_path.clone(),
        });
        let snapshot = build_rule_index(index_root, rule)?;
        snapshots.insert(rule.id.clone(), Arc::new(snapshot));
    }
    Ok(BuiltState {
        rules: indexed_rules,
        snapshots,
    })
}

fn build_rule_index(index_root: &Path, rule: &FileRule) -> Result<RuleSnapshot> {
    let rule_base = index_root.join(sanitize_id(&rule.id));
    migrate_legacy_layout(&rule_base)?;

    let prev = load_previous_generation(&rule_base)?;
    let generation = alloc_generation(&rule_base, prev.as_ref().map(|p| p.generation))?;

    let work_dir = rule_base.join(GEN_DIR).join(generation.to_string());
    fs::create_dir_all(&work_dir)?;

    let db_path = work_dir.join("index.db");
    let bloom_path = work_dir.join("bloom.bin");
    let bit_count = rule.index.bloom_megabytes * 1024 * 1024 * 8;
    let mut bloom = BloomFilter::new(bit_count.max(1 << 16));

    let conn = Connection::open(&db_path)?;
    conn.execute_batch(
        "CREATE TABLE signatures (
            sig_hash INTEGER NOT NULL,
            path TEXT NOT NULL,
            byte_offset INTEGER NOT NULL,
            byte_len INTEGER NOT NULL
        );
        CREATE INDEX idx_sig_hash ON signatures(sig_hash);",
    )?;
    drop(conn);

    let file_paths = collect_files(rule)?;
    let file_count = file_paths.len();
    let indexed_paths: Arc<HashSet<String>> = Arc::new(
        file_paths
            .iter()
            .map(|p| normalize_index_path(p))
            .collect(),
    );
    let (unchanged, to_index, new_manifest) =
        classify_file_changes(&file_paths, prev.as_ref().map(|p| &p.files))?;
    let skipped = unchanged.len();
    let reindexed = to_index.len();

    if let Some(prev_gen) = &prev {
        let unchanged_paths: Vec<String> = unchanged
            .iter()
            .map(|p| normalize_index_path(p))
            .collect();
        let copied = copy_signatures_from_db(&db_path, &prev_gen.db_path, &unchanged_paths)?;
        tracing::debug!(
            rule_id = %rule.id,
            copied,
            skipped,
            "copied signatures from previous generation"
        );
    }

    let _sig_added = index_file_batch(&db_path, &to_index, rule)?;
    let sig_count = count_signatures(&db_path)?;

    {
        let conn = Connection::open(&db_path)?;
        let mut stmt = conn.prepare("SELECT sig_hash FROM signatures")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let h: i64 = row.get(0)?;
            bloom.insert_hash(h as u64);
        }
    }
    bloom.save(&bloom_path)?;

    let files_manifest = FilesManifest {
        files: new_manifest.into_values().collect(),
    };
    fs::write(
        work_dir.join("files.json"),
        serde_json::to_string(&files_manifest)?,
    )?;

    let manifest = RuleManifest {
        generation,
        signatures: sig_count,
        files: file_count,
        skipped,
        reindexed,
    };
    fs::write(
        work_dir.join("manifest.json"),
        serde_json::to_string(&manifest)?,
    )?;

    let literals = if rule.index.scan_rg_prefilter {
        build_scan_literals(&db_path, rule, rule.index.scan_rg_literals_max)?
    } else {
        Vec::new()
    };
    fs::write(
        work_dir.join(LITERALS_FILE),
        serde_json::to_string(&literals)?,
    )?;

    write_current_pointer(&rule_base, generation)?;
    cleanup_old_generations(&rule_base, generation)?;

    tracing::info!(
        rule_id = %rule.id,
        generation,
        signatures = sig_count,
        files = file_count,
        skipped,
        reindexed,
        "file index built (incremental)"
    );

    Ok(RuleSnapshot {
        generation,
        bloom,
        db_path,
        literals: Arc::new(literals),
        indexed_paths,
    })
}

fn next_generation(prev: Option<u64>) -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(1);
    match prev {
        Some(g) if now <= g => g + 1,
        _ => now,
    }
}

fn alloc_generation(rule_base: &Path, prev: Option<u64>) -> Result<u64> {
    let mut gen = next_generation(prev);
    loop {
        let work_dir = rule_base.join(GEN_DIR).join(gen.to_string());
        if !work_dir.exists() {
            return Ok(gen);
        }
        gen += 1;
    }
}

fn migrate_legacy_layout(rule_base: &Path) -> Result<()> {
    let legacy_db = rule_base.join("index.db");
    if legacy_db.exists() && !rule_base.join(CURRENT_FILE).exists() {
        fs::remove_dir_all(rule_base).context("remove legacy flat index layout")?;
    }
    Ok(())
}

fn load_previous_generation(rule_base: &Path) -> Result<Option<PreviousGeneration>> {
    let current_path = rule_base.join(CURRENT_FILE);
    if !current_path.exists() {
        return Ok(None);
    }
    let current: CurrentPointer =
        serde_json::from_str(&fs::read_to_string(&current_path).context("read current.json")?)?;
    let work_dir = rule_base.join(GEN_DIR).join(current.generation.to_string());
    let db_path = work_dir.join("index.db");
    let files_path = work_dir.join("files.json");
    if !db_path.exists() || !files_path.exists() {
        return Ok(None);
    }
    let files_manifest: FilesManifest =
        serde_json::from_str(&fs::read_to_string(&files_path).context("read files.json")?)?;
    let files = files_manifest
        .files
        .into_iter()
        .map(|f| (f.path.clone(), f))
        .collect();
    Ok(Some(PreviousGeneration {
        generation: current.generation,
        db_path,
        files,
    }))
}

fn classify_file_changes(
    paths: &[PathBuf],
    prev: Option<&HashMap<String, FileFingerprint>>,
) -> Result<(Vec<PathBuf>, Vec<PathBuf>, HashMap<String, FileFingerprint>)> {
    let mut unchanged = Vec::new();
    let mut to_index = Vec::new();
    let mut new_manifest = HashMap::new();
    for path in paths {
        let fp = fingerprint_file(path)?;
        new_manifest.insert(fp.path.clone(), fp.clone());
        let same = prev
            .and_then(|m| m.get(&fp.path))
            .is_some_and(|old| old == &fp);
        if same {
            unchanged.push(path.clone());
        } else {
            to_index.push(path.clone());
        }
    }
    Ok((unchanged, to_index, new_manifest))
}

fn fingerprint_file(path: &Path) -> Result<FileFingerprint> {
    let meta = fs::metadata(path)?;
    let modified = meta
        .modified()
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    Ok(FileFingerprint {
        path: path.to_string_lossy().replace('\\', "/"),
        mtime_secs: modified.as_secs(),
        size: meta.len(),
    })
}

fn copy_signatures_from_db(new_db: &Path, old_db: &Path, paths: &[String]) -> Result<u64> {
    if paths.is_empty() || !old_db.exists() {
        return Ok(0);
    }
    let conn = Connection::open(new_db)?;
    let old_path = old_db.to_string_lossy().replace('\'', "''");
    conn.execute_batch(&format!("ATTACH DATABASE '{old_path}' AS old_db"))?;
    let mut copied = 0u64;
    for path in paths {
        let n = conn.execute(
            "INSERT INTO signatures (sig_hash, path, byte_offset, byte_len)
             SELECT sig_hash, path, byte_offset, byte_len FROM old_db.signatures WHERE path = ?1",
            params![path],
        )?;
        copied += n as u64;
    }
    conn.execute_batch("DETACH old_db")?;
    Ok(copied)
}

fn index_file_batch(db_path: &Path, files: &[PathBuf], rule: &FileRule) -> Result<u64> {
    if files.is_empty() {
        return Ok(0);
    }
    let before = count_signatures(db_path)?;
    let workers = rule.index.build_workers.max(1).min(16);
    let (batch_tx, batch_rx) = mpsc::sync_channel::<Vec<SigRow>>(workers * 4);
    let db_path_writer = db_path.to_path_buf();
    let files_arc = Arc::new(files.to_vec());

    let writer = thread::spawn(move || -> Result<u64> {
        let conn = Connection::open(&db_path_writer)?;
        let mut count = 0u64;
        while let Ok(batch) = batch_rx.recv() {
            if batch.is_empty() {
                break;
            }
            let tx = conn.unchecked_transaction()?;
            {
                let mut stmt = tx.prepare(
                    "INSERT INTO signatures (sig_hash, path, byte_offset, byte_len) VALUES (?1, ?2, ?3, ?4)",
                )?;
                for row in &batch {
                    stmt.execute(params![
                        row.hash as i64,
                        row.path,
                        row.byte_offset as i64,
                        row.byte_len as i64
                    ])?;
                    count += 1;
                }
            }
            tx.commit()?;
        }
        Ok(count)
    });

    let mut file_workers = Vec::new();
    for worker_id in 0..workers {
        let files_w = files_arc.clone();
        let batch_tx = batch_tx.clone();
        let rule_w = rule.clone();
        file_workers.push(thread::spawn(move || {
            let mut idx = worker_id;
            while idx < files_w.len() {
                let path = &files_w[idx];
                if let Err(e) = index_one_file(path, &rule_w, &batch_tx) {
                    tracing::warn!(path = %path.display(), error = %e, "index file failed");
                }
                idx += workers;
            }
        }));
    }
    let shutdown_tx = batch_tx.clone();
    drop(batch_tx);
    for h in file_workers {
        let _ = h.join();
    }
    let _ = shutdown_tx.send(Vec::new());
    let _ = writer.join().unwrap_or(Ok(0))?;
    let after = count_signatures(db_path)?;
    Ok(after.saturating_sub(before))
}

fn count_signatures(db_path: &Path) -> Result<u64> {
    let conn = Connection::open(db_path)?;
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM signatures", [], |r| r.get(0))?;
    Ok(n as u64)
}

fn write_current_pointer(rule_base: &Path, generation: u64) -> Result<()> {
    fs::create_dir_all(rule_base)?;
    let tmp = rule_base.join(format!("{CURRENT_FILE}.tmp"));
    let pointer = CurrentPointer { generation };
    fs::write(&tmp, serde_json::to_string(&pointer)?)?;
    fs::rename(tmp, rule_base.join(CURRENT_FILE))?;
    Ok(())
}

fn cleanup_old_generations(rule_base: &Path, keep: u64) -> Result<()> {
    let gen_root = rule_base.join(GEN_DIR);
    if !gen_root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(&gen_root)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if name_str != keep.to_string() {
            fs::remove_dir_all(entry.path()).ok();
        }
    }
    Ok(())
}

fn sanitize_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn collect_files(rule: &FileRule) -> Result<Vec<PathBuf>> {
    let path = &rule.path;
    if !path.exists() {
        tracing::warn!(path = %path.display(), "file rule path does not exist");
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    if path.is_file() {
        out.push(path.clone());
        return Ok(out);
    }
    let walker = if rule.recursive {
        WalkDir::new(path).into_iter()
    } else {
        WalkDir::new(path).max_depth(1).into_iter()
    };
    for entry in walker.filter_map(|e| e.ok()) {
        let p = entry.path();
        if p.is_file() && matches_format(p, &rule.formats) {
            out.push(p.to_path_buf());
        }
    }
    Ok(out)
}

fn matches_format(path: &Path, formats: &[String]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| formats.iter().any(|f| f.eq_ignore_ascii_case(ext)))
        .unwrap_or(false)
}

fn index_one_file(path: &Path, rule: &FileRule, tx: &SyncSender<Vec<SigRow>>) -> Result<()> {
    let meta = fs::metadata(path)?;
    let size = meta.len();
    let path_str = normalize_index_path(path);

    if size <= rule.index.max_full_file_bytes && rule.match_mode == MatchMode::Full {
        let data = read_file_bytes(path, size)?;
        if data.is_empty() {
            return Ok(());
        }
        let mut rows = Vec::new();
        push_chunk_signatures(&data, 0, &path_str, rule, &mut rows);
        if !rows.is_empty() {
            let _ = tx.send(rows);
        }
        return Ok(());
    }

    if size <= INDEX_READ_CAP {
        if let Some(text) = doc_extract::extract_text(path).ok() {
            if !text.is_empty() {
                let bytes = text.into_bytes();
                let mut rows = Vec::new();
                index_bytes_chunks(&bytes, 0, &path_str, rule, &mut rows);
                if !rows.is_empty() {
                    let _ = tx.send(rows);
                }
                return Ok(());
            }
        }
        let data = read_file_bytes(path, size)?;
        let mut rows = Vec::new();
        index_bytes_chunks(&data, 0, &path_str, rule, &mut rows);
        if !rows.is_empty() {
            let _ = tx.send(rows);
        }
        return Ok(());
    }

    stream_index_file(path, &path_str, rule, tx)
}

fn read_file_bytes(path: &Path, size: u64) -> Result<Vec<u8>> {
    let mut f = File::open(path)?;
    let mut buf = vec![0u8; size as usize];
    f.read_exact(&mut buf)?;
    Ok(buf)
}

fn stream_index_file(path: &Path, path_str: &str, rule: &FileRule, tx: &SyncSender<Vec<SigRow>>) -> Result<()> {
    let mut f = File::open(path)?;
    let chunk_size = rule.index.chunk_size;
    let overlap = rule.index.chunk_overlap;
    let step = chunk_size.saturating_sub(overlap).max(1);
    let mut carry = Vec::new();
    let mut file_offset: u64 = 0;
    let mut read_buf = vec![0u8; 1024 * 1024];
    let mut batch = Vec::new();

    loop {
        let n = f.read(&mut read_buf)?;
        if n == 0 {
            break;
        }
        carry.extend_from_slice(&read_buf[..n]);
        while carry.len() >= chunk_size {
            push_chunk_signatures(&carry[..chunk_size], file_offset, path_str, rule, &mut batch);
            if batch.len() >= 5000 {
                let _ = tx.send(std::mem::take(&mut batch));
            }
            if carry.len() <= step {
                carry.clear();
                file_offset += step as u64;
                break;
            }
            carry.drain(..step);
            file_offset += step as u64;
        }
    }
    if !carry.is_empty() {
        push_chunk_signatures(&carry, file_offset, path_str, rule, &mut batch);
    }
    if !batch.is_empty() {
        let _ = tx.send(batch);
    }
    Ok(())
}

fn index_bytes_chunks(data: &[u8], base_offset: u64, path_str: &str, rule: &FileRule, rows: &mut Vec<SigRow>) {
    let chunk_size = rule.index.chunk_size;
    let overlap = rule.index.chunk_overlap;
    let step = chunk_size.saturating_sub(overlap).max(1);
    let mut offset = 0usize;
    while offset < data.len() {
        let end = (offset + chunk_size).min(data.len());
        push_chunk_signatures(&data[offset..end], base_offset + offset as u64, path_str, rule, rows);
        if end >= data.len() {
            break;
        }
        offset += step;
    }
}

fn push_chunk_signatures(
    chunk: &[u8],
    byte_offset: u64,
    path: &str,
    rule: &FileRule,
    rows: &mut Vec<SigRow>,
) {
    if chunk.len() < 4 {
        return;
    }
    let h0 = xxh64(chunk, 0);
    rows.push(SigRow {
        hash: h0,
        path: path.to_string(),
        byte_offset,
        byte_len: chunk.len() as u32,
    });

    let min_len = effective_min_fragment_len(
        chunk.len(),
        rule.min_fragment_len,
        rule.min_fragment_ratio,
    )
    .max(4)
    .min(chunk.len());

    let stride = rule.index.signature_stride.max(1);
    let max_sigs = rule.index.signatures_per_chunk;
    let lens = signature_lengths(min_len, chunk.len());

    let mut emitted = 0usize;
    let mut pos = 0usize;
    while pos + min_len <= chunk.len() && emitted < max_sigs {
        for &len in &lens {
            if pos + len > chunk.len() {
                continue;
            }
            let slice = &chunk[pos..pos + len];
            rows.push(SigRow {
                hash: xxh64(slice, 0),
                path: path.to_string(),
                byte_offset: byte_offset + pos as u64,
                byte_len: len as u32,
            });
            emitted += 1;
            if emitted >= max_sigs {
                break;
            }
        }
        pos += stride;
    }
}

fn signature_lengths(min_len: usize, chunk_len: usize) -> Vec<usize> {
    let mut lens = vec![min_len];
    for &candidate in &[64usize, 256, 1024, 8192] {
        if candidate >= min_len && candidate <= chunk_len && !lens.contains(&candidate) {
            lens.push(candidate);
        }
    }
    if chunk_len > min_len && !lens.contains(&chunk_len) {
        lens.push(chunk_len);
    }
    lens
}

fn normalize_index_path(path: &Path) -> String {
    let raw = if path.is_file() {
        if let Ok(canon) = fs::canonicalize(path) {
            canon.to_string_lossy().replace('\\', "/")
        } else {
            path.to_string_lossy().replace('\\', "/")
        }
    } else {
        path.to_string_lossy().replace('\\', "/")
    };
    strip_verbatim_path_prefix(&raw)
}

fn scan_haystack(
    haystack: &str,
    rule: &FileRule,
    snapshot: &RuleSnapshot,
    allowed_paths: &HashSet<String>,
) -> String {
    if allowed_paths.is_empty() {
        return haystack.to_string();
    }
    if haystack.is_empty() {
        return haystack.to_string();
    }
    if haystack.len() > rule.index.max_haystack_bytes {
        tracing::warn!(
            len = haystack.len(),
            max = rule.index.max_haystack_bytes,
            "haystack exceeds max_haystack_bytes; truncating scan"
        );
    }
    let scan_len = haystack.len().min(rule.index.max_haystack_bytes);
    let hay = &haystack[..scan_len];
    let bytes = hay.as_bytes();

    let min_len = effective_min_fragment_len(
        rule.index.chunk_size,
        rule.min_fragment_len,
        rule.min_fragment_ratio,
    )
    .max(4);

    let lens = signature_lengths(min_len, bytes.len().min(rule.index.chunk_size));

    let Ok(conn) = Connection::open_with_flags(
        &snapshot.db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) else {
        return haystack.to_string();
    };

    let mut byte_ranges = Vec::new();

    if rule.index.scan_rg_prefilter
        && bytes.len() >= RG_PREFILTER_MIN_BYTES
        && !snapshot.literals.is_empty()
    {
        byte_ranges.extend(rg_prefilter_ranges(
            bytes,
            snapshot.literals.as_ref(),
            &snapshot.bloom,
            &conn,
            allowed_paths,
        ));
    }

    byte_ranges.extend(parallel_bloom_scan(
        bytes,
        rule,
        snapshot,
        &lens,
        allowed_paths,
    ));

    if byte_ranges.is_empty() {
        return haystack.to_string();
    }

    byte_ranges.sort_by_key(|r| r.0);
    let merged = merge_byte_ranges(byte_ranges);
    apply_byte_ranges(haystack, &merged, rule.match_mode)
}

fn rg_prefilter_ranges(
    bytes: &[u8],
    literals: &[String],
    bloom: &BloomFilter,
    conn: &Connection,
    allowed_paths: &HashSet<String>,
) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    for lit in literals {
        if lit.is_empty() {
            continue;
        }
        let lit_bytes = lit.as_bytes();
        for pos in find_literal_byte_offsets(bytes, lit_bytes) {
            let end = pos + lit_bytes.len();
            if end > bytes.len() {
                continue;
            }
            let candidate = &bytes[pos..end];
            let h = xxh64(candidate, 0);
            if bloom.may_contain(h) && verify_candidate(conn, h, candidate, allowed_paths) {
                ranges.push((pos, end));
            }
        }
    }
    ranges
}

fn parallel_bloom_scan(
    bytes: &[u8],
    rule: &FileRule,
    snapshot: &RuleSnapshot,
    lens: &[usize],
    allowed_paths: &HashSet<String>,
) -> Vec<(usize, usize)> {
    let chunk_size = rule.index.chunk_size;
    let overlap = rule.index.chunk_overlap;
    let step = chunk_size.saturating_sub(overlap).max(1);
    let workers = rule.index.scan_workers.max(1).min(16);
    let starts: Vec<usize> = (0..bytes.len()).step_by(step).collect();

    if bytes.len() <= chunk_size || workers == 1 || starts.len() <= 1 {
        return scan_region_bloom(
            bytes,
            0,
            lens,
            &snapshot.bloom,
            &snapshot.db_path,
            allowed_paths,
        );
    }

    let bytes = Arc::new(bytes.to_vec());
    let db_path = snapshot.db_path.clone();
    let bloom = snapshot.bloom.clone();
    let lens = Arc::new(lens.to_vec());
    let allowed = Arc::new(allowed_paths.clone());
    let (tx, rx) = mpsc::channel::<Vec<(usize, usize)>>();

    for worker_id in 0..workers {
        let tx = tx.clone();
        let bytes = bytes.clone();
        let db_path = db_path.clone();
        let bloom = bloom.clone();
        let lens = lens.clone();
        let starts = starts.clone();
        let allowed = allowed.clone();
        thread::spawn(move || {
            let Ok(conn) = Connection::open_with_flags(
                &db_path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            ) else {
                let _ = tx.send(Vec::new());
                return;
            };
            let mut local = Vec::new();
            let mut idx = worker_id;
            while idx < starts.len() {
                let start = starts[idx];
                let end = (start + chunk_size).min(bytes.len());
                local.extend(scan_region_bloom_slice(
                    &bytes[start..end],
                    start,
                    &lens,
                    &bloom,
                    &conn,
                    allowed.as_ref(),
                ));
                idx += workers;
            }
            let _ = tx.send(local);
        });
    }
    drop(tx);

    let mut all = Vec::new();
    while let Ok(mut batch) = rx.recv() {
        all.append(&mut batch);
    }
    all
}

fn scan_region_bloom(
    bytes: &[u8],
    base_offset: usize,
    lens: &[usize],
    bloom: &BloomFilter,
    db_path: &Path,
    allowed_paths: &HashSet<String>,
) -> Vec<(usize, usize)> {
    let Ok(conn) = Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return Vec::new();
    };
    scan_region_bloom_slice(
        bytes,
        base_offset,
        lens,
        bloom,
        &conn,
        allowed_paths,
    )
}

fn scan_region_bloom_slice(
    bytes: &[u8],
    base_offset: usize,
    lens: &[usize],
    bloom: &BloomFilter,
    conn: &Connection,
    allowed_paths: &HashSet<String>,
) -> Vec<(usize, usize)> {
    let mut byte_ranges = Vec::new();
    let mut pos = 0usize;
    while pos < bytes.len() {
        for &len in lens {
            if pos + len > bytes.len() {
                continue;
            }
            let candidate = &bytes[pos..pos + len];
            let h = xxh64(candidate, 0);
            if !bloom.may_contain(h) {
                continue;
            }
            if verify_candidate(conn, h, candidate, allowed_paths) {
                byte_ranges.push((base_offset + pos, base_offset + pos + len));
            }
        }
        pos += 1;
    }
    byte_ranges
}

fn build_scan_literals(db_path: &Path, rule: &FileRule, cap: usize) -> Result<Vec<String>> {
    if cap == 0 {
        return Ok(Vec::new());
    }
    let min_len = effective_min_fragment_len(
        rule.index.chunk_size,
        rule.min_fragment_len,
        rule.min_fragment_ratio,
    )
    .max(4);
    let max_len = 512usize;
    let conn = Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut stmt = conn.prepare(
        "SELECT path, byte_offset, byte_len FROM signatures
         WHERE byte_len >= ?1 AND byte_len <= ?2
         ORDER BY sig_hash
         LIMIT ?3",
    )?;
    let fetch = cap.saturating_mul(4).min(100_000);
    let mut rows = stmt.query(params![min_len as i64, max_len as i64, fetch as i64])?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let path: String = row.get(0)?;
        let offset: i64 = row.get(1)?;
        let len: i32 = row.get(2)?;
        let Some(lit) = read_literal_string(&path, offset as u64, len as u32) else {
            continue;
        };
        if lit.len() >= min_len && seen.insert(lit.clone()) {
            out.push(lit);
            if out.len() >= cap {
                break;
            }
        }
    }
    Ok(out)
}

fn read_literal_string(path: &str, offset: u64, len: u32) -> Option<String> {
    let mut f = File::open(path).ok()?;
    let mut buf = vec![0u8; len as usize];
    f.seek(std::io::SeekFrom::Start(offset)).ok()?;
    f.read_exact(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn verify_candidate(
    conn: &Connection,
    hash: u64,
    candidate: &[u8],
    allowed_paths: &HashSet<String>,
) -> bool {
    let mut stmt = match conn.prepare(
        "SELECT path, byte_offset, byte_len FROM signatures WHERE sig_hash = ?1 LIMIT 16",
    ) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let mut rows = match stmt.query(params![hash as i64]) {
        Ok(r) => r,
        Err(_) => return false,
    };
    while let Ok(Some(row)) = rows.next() {
        let path: String = row.get(0).unwrap_or_default();
        if !allowed_paths.contains(&path) {
            continue;
        }
        let offset: i64 = row.get(1).unwrap_or(0);
        let len: i32 = row.get(2).unwrap_or(0);
        if len <= 0 {
            continue;
        }
        if let Ok(mut f) = File::open(&path) {
            let mut buf = vec![0u8; len as usize];
            if f.seek(std::io::SeekFrom::Start(offset as u64)).is_ok()
                && f.read_exact(&mut buf).is_ok()
                && buf == candidate
            {
                return true;
            }
        }
    }
    false
}

fn merge_byte_ranges(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    if ranges.is_empty() {
        return ranges;
    }
    ranges.sort_by_key(|r| r.0);
    let mut out = vec![ranges[0]];
    for (s, e) in ranges.into_iter().skip(1) {
        let last = out.last_mut().unwrap();
        if s <= last.1 {
            last.1 = last.1.max(e);
        } else {
            out.push((s, e));
        }
    }
    out
}

fn apply_byte_ranges(text: &str, ranges: &[(usize, usize)], mode: MatchMode) -> String {
    let mut char_ranges: Vec<(usize, usize)> = Vec::new();
    for &(b_start, b_end) in ranges {
        if b_start >= text.len() || b_end > text.len() || b_start >= b_end {
            continue;
        }
        let prefix = &text[..b_start];
        let slice = &text[b_start..b_end];
        let cs = prefix.chars().count();
        let ce = cs + slice.chars().count();
        char_ranges.push((cs, ce));
    }
    char_ranges.sort_by_key(|r| r.0);
    let merged = merge_char_ranges(char_ranges);
    let mut result = text.to_string();
    for (start, end) in merged.into_iter().rev() {
        if mode == MatchMode::Full {
            let matched: String = result.chars().skip(start).take(end - start).collect();
            let rep = sanitize_whole(&matched);
            result = replace_char_range(&result, start, end, &rep);
        } else {
            result = sanitize_range(&result, start, end);
        }
    }
    result
}

fn merge_char_ranges(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    if ranges.is_empty() {
        return ranges;
    }
    ranges.sort_by_key(|r| r.0);
    let mut out = vec![ranges[0]];
    for (s, e) in ranges.into_iter().skip(1) {
        let last = out.last_mut().unwrap();
        if s <= last.1 {
            last.1 = last.1.max(e);
        } else {
            out.push((s, e));
        }
    }
    out
}

fn replace_char_range(text: &str, start: usize, end: usize, replacement: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out: String = chars.iter().take(start).collect();
    out.push_str(replacement);
    out.extend(chars.iter().skip(end));
    out
}

pub fn filter_most_specific_rules(rules: &[IndexedRule], tool_text: &str) -> Vec<FileRule> {
    let matches: Vec<&IndexedRule> = rules
        .iter()
        .filter(|r| path_trigger_match(&r.normalized_path, tool_text))
        .collect();
    if matches.is_empty() {
        return Vec::new();
    }
    let match_paths: Vec<&str> = matches.iter().map(|m| m.normalized_path.as_str()).collect();
    matches
        .into_iter()
        .filter(|candidate| {
            let p = candidate.normalized_path.as_str();
            !match_paths.iter().any(|other| {
                *other != p
                    && other.starts_with(p)
                    && other.as_bytes().get(p.len()) == Some(&b'/')
            })
        })
        .map(|r| r.rule.clone())
        .collect()
}

pub fn check_path_in_rules(rules: &[IndexedRule], tool_text: &str) -> Vec<FileRule> {
    filter_most_specific_rules(rules, tool_text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::FileIndexOptions;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn builds_and_scans_small_dir() {
        let tmp = TempDir::new().unwrap();
        let secret = tmp.path().join("secret.txt");
        let mut f = File::create(&secret).unwrap();
        writeln!(f, "TOP-SECRET-PHRASE-XYZZY").unwrap();

        let rule = FileRule {
            id: "t1".into(),
            path: tmp.path().to_path_buf(),
            enabled: true,
            recursive: true,
            trigger_window: 3,
            match_mode: MatchMode::Fragment,
            min_fragment_len: Some(8),
            min_fragment_ratio: None,
            formats: vec!["txt".into()],
            index: FileIndexOptions {
                bloom_megabytes: 1,
                build_workers: 2,
                ..Default::default()
            },
        };

        let index_root = tmp.path().join("idx");
        let snapshot = build_rule_index(&index_root, &rule).unwrap();
        let hay = "user pasted TOP-SECRET-PHRASE-XYZZY here";
        let allowed: HashSet<String> = snapshot.indexed_paths.iter().cloned().collect();
        let out = scan_haystack(hay, &rule, &snapshot, &allowed);
        assert!(!out.contains("TOP-SECRET-PHRASE-XYZZY"));
        assert_eq!(out.chars().count(), hay.chars().count());
    }

    #[test]
    fn incremental_rebuild_skips_unchanged_files() {
        let tmp = TempDir::new().unwrap();
        let a = tmp.path().join("a.txt");
        let b = tmp.path().join("b.txt");
        fs::write(&a, "ALPHA-SECRET-ONE").unwrap();
        fs::write(&b, "BETA-SECRET-TWO").unwrap();

        let rule = FileRule {
            id: "inc".into(),
            path: tmp.path().to_path_buf(),
            enabled: true,
            recursive: true,
            trigger_window: 3,
            match_mode: MatchMode::Fragment,
            min_fragment_len: Some(8),
            min_fragment_ratio: None,
            formats: vec!["txt".into()],
            index: FileIndexOptions {
                bloom_megabytes: 1,
                build_workers: 2,
                ..Default::default()
            },
        };

        let index_root = tmp.path().join("idx");
        let snap1 = build_rule_index(&index_root, &rule).unwrap();
        let gen1 = snap1.generation;

        let snap2 = build_rule_index(&index_root, &rule).unwrap();
        assert!(snap2.generation >= gen1);
        let manifest_path = index_root
            .join("inc")
            .join(GEN_DIR)
            .join(snap2.generation.to_string())
            .join("manifest.json");
        let manifest: RuleManifest =
            serde_json::from_str(&fs::read_to_string(manifest_path).unwrap()).unwrap();
        assert_eq!(manifest.files, 2);
        assert_eq!(manifest.skipped, 2);
        assert_eq!(manifest.reindexed, 0);

        fs::write(&a, "ALPHA-SECRET-ONE-MODIFIED").unwrap();
        let snap3 = build_rule_index(&index_root, &rule).unwrap();
        let manifest_path = index_root
            .join("inc")
            .join(GEN_DIR)
            .join(snap3.generation.to_string())
            .join("manifest.json");
        let manifest: RuleManifest =
            serde_json::from_str(&fs::read_to_string(manifest_path).unwrap()).unwrap();
        assert_eq!(manifest.skipped, 1);
        assert_eq!(manifest.reindexed, 1);

        let hay = "leak ALPHA-SECRET-ONE-MODIFIED end";
        let allowed: HashSet<String> = snap3.indexed_paths.iter().cloned().collect();
        let out = scan_haystack(hay, &rule, &snap3, &allowed);
        assert!(!out.contains("ALPHA-SECRET-ONE-MODIFIED"));
    }

    #[test]
    fn parallel_scan_finds_secret_in_large_haystack() {
        let tmp = TempDir::new().unwrap();
        let secret = tmp.path().join("secret.txt");
        fs::write(&secret, "WIDE-CORPUS-LEAK-TOKEN-99").unwrap();

        let rule = FileRule {
            id: "wide".into(),
            path: tmp.path().to_path_buf(),
            enabled: true,
            recursive: true,
            trigger_window: 3,
            match_mode: MatchMode::Fragment,
            min_fragment_len: Some(8),
            min_fragment_ratio: None,
            formats: vec!["txt".into()],
            index: FileIndexOptions {
                bloom_megabytes: 1,
                build_workers: 2,
                scan_workers: 4,
                scan_rg_prefilter: true,
                scan_rg_literals_max: 256,
                ..Default::default()
            },
        };

        let index_root = tmp.path().join("idx");
        let snapshot = build_rule_index(&index_root, &rule).unwrap();
        assert!(!snapshot.literals.is_empty());

        let padding = "x".repeat(20_000);
        let hay = format!("{padding} WIDE-CORPUS-LEAK-TOKEN-99 {padding}");
        let allowed: HashSet<String> = snapshot.indexed_paths.iter().cloned().collect();
        let out = scan_haystack(&hay, &rule, &snapshot, &allowed);
        assert!(!out.contains("WIDE-CORPUS-LEAK-TOKEN-99"));
        assert_eq!(out.chars().count(), hay.chars().count());
    }

    #[test]
    fn scoped_scan_only_triggered_file_in_directory() {
        let tmp = TempDir::new().unwrap();
        let triggered = tmp.path().join("triggered.txt");
        let other = tmp.path().join("other.txt");
        fs::write(&triggered, "TRIGGERED-ONLY-SECRET-ABC").unwrap();
        fs::write(&other, "OTHER-FILE-SECRET-XYZ").unwrap();

        let rule = FileRule {
            id: "dir".into(),
            path: tmp.path().to_path_buf(),
            enabled: true,
            recursive: true,
            trigger_window: 3,
            match_mode: MatchMode::Fragment,
            min_fragment_len: Some(8),
            min_fragment_ratio: None,
            formats: vec!["txt".into()],
            index: FileIndexOptions {
                bloom_megabytes: 1,
                build_workers: 1,
                scan_workers: 1,
                scan_rg_prefilter: false,
                ..Default::default()
            },
        };

        let index_root = tmp.path().join("idx");
        let snapshot = build_rule_index(&index_root, &rule).unwrap();
        let triggered_path = normalize_index_path(&triggered);
        let allowed: HashSet<String> = HashSet::from([triggered_path]);

        let hay_triggered = "leak TRIGGERED-ONLY-SECRET-ABC here";
        let out1 = scan_haystack(hay_triggered, &rule, &snapshot, &allowed);
        assert!(!out1.contains("TRIGGERED-ONLY-SECRET-ABC"));

        let hay_other = "leak OTHER-FILE-SECRET-XYZ here";
        let out2 = scan_haystack(hay_other, &rule, &snapshot, &allowed);
        assert!(out2.contains("OTHER-FILE-SECRET-XYZ"));
    }
}
