//! Disk-backed file index: SQLite signatures + Bloom pre-filter (P0–P2 large corpus).

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek};
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
use crate::dlp::charset::{
    accumulate_token_profile, should_token_prefilter_skip, token_prefilter_applicable,
    token_profile, TokenProfile,
};
use crate::dlp::doc_extract;
use crate::dlp::file::{path_basename, path_trigger_match, paths_equivalent, strip_verbatim_path_prefix};
use crate::dlp::fragment::{file_fragment_meets_threshold, file_min_fragment_len};
use crate::dlp::normalize::{normalize_with_map, Normalized};
use crate::dlp::sanitize::{sanitize_range, sanitize_whole};
use crate::dlp::session::ActiveFileContent;
use crate::paths;

pub const INDEX_READ_CAP: u64 = 16 * 1024 * 1024;

const GEN_DIR: &str = "gen";
const CURRENT_FILE: &str = "current.json";
const LITERALS_FILE: &str = "literals.json";
/// Haystack length at which ripgrep literal prefilter is enabled.
const RG_PREFILTER_MIN_BYTES: usize = 8192;
/// Bytes read per indexed file when building token fingerprints.
const PATH_TOKEN_SAMPLE_BYTES: usize = 256 * 1024;

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
    path_tokens: Arc<HashMap<String, TokenProfile>>,
}

struct IndexState {
    ready: AtomicBool,
    rebuilding: AtomicBool,
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
            rebuilding: AtomicBool::new(false),
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
            {
                let guard = inner.write();
                guard.ready.store(false, Ordering::Release);
                guard.rebuilding.store(true, Ordering::Release);
            }
            let build_result = build_all_rules(&index_root, &rules_vec);
            if !alive.load(Ordering::Acquire) {
                return;
            }
            if let Ok(state) = build_result {
                let mut guard = inner.write();
                guard.rules = state.rules;
                guard.snapshots = state.snapshots;
                guard.rebuilding.store(false, Ordering::Release);
                guard.ready.store(true, Ordering::Release);
            } else {
                tracing::error!("file index build failed");
                inner.write().rebuilding.store(false, Ordering::Release);
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
        {
            let guard = self.inner.write();
            guard.ready.store(false, Ordering::Release);
            guard.rebuilding.store(true, Ordering::Release);
        }
        let state = build_all_rules(&self.index_root, rules)?;
        let mut guard = self.inner.write();
        guard.rules = state.rules;
        guard.snapshots = state.snapshots;
        guard.rebuilding.store(false, Ordering::Release);
        guard.ready.store(true, Ordering::Release);
        Ok(())
    }

    pub fn scan_and_sanitize(
        &self,
        text: &str,
        active: &[ActiveFileContent],
        vault: Option<(&str, &crate::dlp::TokenVault)>,
    ) -> String {
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
            result = scan_haystack(&result, &item.rule, snapshot, &allowed, vault);
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
        let paths = dedupe_nested_watch_paths(
            rules
                .iter()
                .filter(|r| r.enabled)
                .map(|r| r.path.clone())
                .collect(),
        );
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
                thread::sleep(std::time::Duration::from_millis(5000));
                while rx.try_recv().is_ok() {}
                if !alive.load(Ordering::Acquire) {
                    break;
                }
                if inner.read().rebuilding.load(Ordering::Acquire) {
                    continue;
                }
                {
                    let guard = inner.write();
                    guard.ready.store(false, Ordering::Release);
                    guard.rebuilding.store(true, Ordering::Release);
                }
                let build_result = build_all_rules(&index_root, &rules_owned);
                if !alive.load(Ordering::Acquire) {
                    break;
                }
                if let Ok(state) = build_result {
                    let mut guard = inner.write();
                    guard.rules = state.rules;
                    guard.snapshots = state.snapshots;
                    guard.rebuilding.store(false, Ordering::Release);
                    guard.ready.store(true, Ordering::Release);
                } else {
                    inner.write().rebuilding.store(false, Ordering::Release);
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
    let (unchanged, to_index, new_manifest) =
        classify_file_changes(&file_paths, prev.as_ref().map(|p| &p.files))?;
    if to_index.is_empty() {
        if let Some(prev_gen) = &prev {
            return load_rule_snapshot(rule, &rule_base, prev_gen, &file_paths);
        }
    }
    let file_count = file_paths.len();
    let indexed_paths: Arc<HashSet<String>> = Arc::new(
        file_paths
            .iter()
            .map(|p| normalize_index_path(p))
            .collect(),
    );
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
        path_tokens: Arc::new(build_path_tokens(&file_paths, rule)),
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

/// Drop nested rule paths so a recursive watch on a parent covers children.
fn dedupe_nested_watch_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut sorted = paths;
    sorted.sort_by_key(|p| p.components().count());
    let mut out: Vec<PathBuf> = Vec::new();
    'next: for path in sorted {
        let norm = normalize_index_path(&path);
        for kept in &out {
            let kept_norm = normalize_index_path(kept);
            if norm == kept_norm || norm.starts_with(&format!("{kept_norm}/")) {
                continue 'next;
            }
        }
        out.push(path);
    }
    out
}

fn load_rule_snapshot(
    rule: &FileRule,
    rule_base: &Path,
    prev_gen: &PreviousGeneration,
    file_paths: &[PathBuf],
) -> Result<RuleSnapshot> {
    let work_dir = rule_base
        .join(GEN_DIR)
        .join(prev_gen.generation.to_string());
    let bloom = BloomFilter::load(&work_dir.join("bloom.bin"))?;
    let literals: Vec<String> = serde_json::from_str(
        &fs::read_to_string(work_dir.join(LITERALS_FILE)).context("read literals.json")?,
    )?;
    let indexed_paths: Arc<HashSet<String>> = Arc::new(
        file_paths
            .iter()
            .map(|p| normalize_index_path(p))
            .collect(),
    );
    tracing::debug!(
        rule_id = %rule.id,
        generation = prev_gen.generation,
        files = file_paths.len(),
        "file index unchanged; reusing snapshot"
    );
    Ok(RuleSnapshot {
        generation: prev_gen.generation,
        bloom,
        db_path: prev_gen.db_path.clone(),
        literals: Arc::new(literals),
        indexed_paths,
        path_tokens: Arc::new(build_path_tokens(file_paths, rule)),
    })
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
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        index_one_file_impl(path, rule, tx)
    })) {
        Ok(result) => result,
        Err(_) => {
            tracing::warn!(path = %path.display(), "index file panicked; skipping");
            Ok(())
        }
    }
}

fn extract_text_safe(path: &Path) -> Option<String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| doc_extract::extract_text(path).ok()))
        .ok()
        .flatten()
}

fn index_one_file_impl(path: &Path, rule: &FileRule, tx: &SyncSender<Vec<SigRow>>) -> Result<()> {
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
        if let Some(text) = extract_text_safe(path) {
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
    if chunk.is_empty() {
        return;
    }
    let chunk_str = String::from_utf8_lossy(chunk);
    let norm = normalize_with_map(&chunk_str);
    if norm.is_empty() {
        return;
    }

    let chunk_start = byte_offset;
    let chunk_len = chunk.len() as u32;

    let h0 = xxh64(norm.text.as_bytes(), 0);
    rows.push(SigRow {
        hash: h0,
        path: path.to_string(),
        byte_offset: chunk_start,
        byte_len: chunk_len,
    });

    let min_len = file_min_fragment_len(rule.min_fragment_len)
        .max(4)
        .min(norm.text.chars().count());

    let stride = rule.index.signature_stride.max(1);
    let max_sigs = rule.index.signatures_per_chunk;
    let char_len = norm.text.chars().count();
    let lens = signature_lengths(min_len, char_len);
    let chars: Vec<char> = norm.text.chars().collect();

    let mut emitted = 0usize;
    let mut pos = 0usize;
    while pos + min_len <= char_len && emitted < max_sigs {
        for &len in &lens {
            if pos + len > char_len {
                continue;
            }
            let slice: String = chars[pos..pos + len].iter().collect();
            rows.push(SigRow {
                hash: xxh64(slice.as_bytes(), 0),
                path: path.to_string(),
                byte_offset: chunk_start,
                byte_len: chunk_len,
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
    for &candidate in &[256usize, 1024, 8192] {
        if candidate >= min_len && candidate <= chunk_len && !lens.contains(&candidate) {
            lens.push(candidate);
        }
    }
    if chunk_len > min_len && !lens.contains(&chunk_len) {
        lens.push(chunk_len);
    }
    lens
}

fn haystack_scan_blocks<'a>(hay: &'a str, chunk_size: usize, overlap: usize) -> Vec<(usize, &'a str)> {
    if hay.is_empty() {
        return Vec::new();
    }
    if hay.len() <= chunk_size {
        return vec![(0, hay)];
    }
    let step = chunk_size.saturating_sub(overlap).max(1);
    let mut blocks = Vec::new();
    let mut start = 0usize;
    while start < hay.len() {
        let end = (start + chunk_size).min(hay.len());
        blocks.push((start, &hay[start..end]));
        if end >= hay.len() {
            break;
        }
        start += step;
    }
    blocks
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

fn build_path_tokens(file_paths: &[PathBuf], rule: &FileRule) -> HashMap<String, TokenProfile> {
    let mut out = HashMap::new();
    for path in file_paths {
        let path_str = normalize_index_path(path);
        let mut tokens = TokenProfile::default();
        collect_file_tokens(path, rule, &mut tokens);
        out.insert(path_str, tokens);
    }
    out
}

fn collect_file_tokens(path: &Path, _rule: &FileRule, out: &mut TokenProfile) {
    let Ok(meta) = fs::metadata(path) else {
        return;
    };
    let size = meta.len();
    if size == 0 {
        return;
    }
    if let Some(text) = extract_text_safe(path).filter(|t| !t.is_empty()) {
        accumulate_token_profile(&text, PATH_TOKEN_SAMPLE_BYTES, out);
        return;
    }
    let read_len = (size as usize).min(PATH_TOKEN_SAMPLE_BYTES);
    if let Ok(data) = read_file_bytes(path, read_len as u64) {
        let text = String::from_utf8_lossy(&data);
        accumulate_token_profile(&text, PATH_TOKEN_SAMPLE_BYTES, out);
    }
}

fn filter_paths_by_tokens(
    allowed: &HashSet<String>,
    path_tokens: &HashMap<String, TokenProfile>,
    hay_tokens: &TokenProfile,
    threshold: f64,
) -> HashSet<String> {
    allowed
        .iter()
        .filter(|path| {
            let Some(file_tokens) = path_tokens.get(*path) else {
                return true;
            };
            if hay_tokens.is_empty() || file_tokens.is_empty() {
                return true;
            }
            if !token_prefilter_applicable(hay_tokens, file_tokens) {
                return true;
            }
            !should_token_prefilter_skip(hay_tokens, file_tokens, threshold)
        })
        .cloned()
        .collect()
}

struct ScanBudget {
    deadline: std::time::Instant,
}

impl ScanBudget {
    fn new(budget_ms: u64) -> Self {
        Self {
            deadline: std::time::Instant::now()
                + std::time::Duration::from_millis(budget_ms.max(1)),
        }
    }

    fn expired(&self) -> bool {
        std::time::Instant::now() >= self.deadline
    }
}

fn scan_haystack(
    haystack: &str,
    rule: &FileRule,
    snapshot: &RuleSnapshot,
    allowed_paths: &HashSet<String>,
    vault: Option<(&str, &crate::dlp::TokenVault)>,
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

    let min_len = file_min_fragment_len(rule.min_fragment_len).max(4);

    let hay_tokens = token_profile(hay);
    let scan_paths = if rule.index.scan_charset_skip {
        let filtered = filter_paths_by_tokens(
            allowed_paths,
            snapshot.path_tokens.as_ref(),
            &hay_tokens,
            rule.index.scan_charset_skip_threshold,
        );
        if filtered.is_empty() {
            tracing::debug!(
                rule_id = %rule.id,
                threshold = rule.index.scan_charset_skip_threshold,
                "token prefilter skipped all triggered paths"
            );
            return haystack.to_string();
        }
        if filtered.len() < allowed_paths.len() {
            tracing::debug!(
                rule_id = %rule.id,
                before = allowed_paths.len(),
                after = filtered.len(),
                "token prefilter narrowed triggered paths"
            );
        }
        filtered
    } else {
        allowed_paths.clone()
    };

    let budget = ScanBudget::new(rule.index.scan_time_budget_ms);

    let Ok(conn) = Connection::open_with_flags(
        &snapshot.db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) else {
        return haystack.to_string();
    };

    let mut byte_ranges = Vec::new();
    let blocks = haystack_scan_blocks(hay, rule.index.chunk_size, rule.index.chunk_overlap);

    for (block_start, block) in blocks {
        if budget.expired() {
            tracing::warn!(
                rule_id = %rule.id,
                budget_ms = rule.index.scan_time_budget_ms,
                "file scan time budget exceeded; stopping early"
            );
            break;
        }
        let block_norm_len = normalize_with_map(block).text.chars().count();
        let lens = signature_lengths(min_len, block_norm_len.max(min_len));

        if rule.index.scan_rg_prefilter
            && block.len() >= RG_PREFILTER_MIN_BYTES
            && !snapshot.literals.is_empty()
        {
            byte_ranges.extend(rg_prefilter_ranges(
                block,
                block_start,
                snapshot.literals.as_ref(),
                &snapshot.bloom,
                &conn,
                &scan_paths,
                rule,
                &budget,
            ));
        }

        if budget.expired() {
            tracing::warn!(
                rule_id = %rule.id,
                budget_ms = rule.index.scan_time_budget_ms,
                "file scan time budget exceeded; stopping early"
            );
            break;
        }

        byte_ranges.extend(scan_block_bloom(
            block,
            block_start,
            rule,
            &lens,
            min_len,
            &snapshot.bloom,
            &snapshot.db_path,
            &scan_paths,
            &budget,
        ));
    }

    if byte_ranges.is_empty() {
        return haystack.to_string();
    }

    byte_ranges.sort_by_key(|r| r.0);
    let merged = merge_byte_ranges(byte_ranges);
    apply_byte_ranges(haystack, &merged, rule.match_mode, vault)
}

fn rg_prefilter_ranges(
    hay_block: &str,
    hay_block_start: usize,
    literals: &[String],
    bloom: &BloomFilter,
    conn: &Connection,
    allowed_paths: &HashSet<String>,
    rule: &FileRule,
    budget: &ScanBudget,
) -> Vec<(usize, usize)> {
    let norm = normalize_with_map(hay_block);
    let hay_chars: Vec<char> = norm.text.chars().collect();
    if hay_chars.len() < file_min_fragment_len(rule.min_fragment_len) {
        return Vec::new();
    }
    let hay_chunk_norm_len = hay_chars.len();
    let mut ranges = Vec::new();
    for lit in literals {
        if budget.expired() {
            break;
        }
        let lit_chars: Vec<char> = lit.chars().collect();
        if lit_chars.len() < file_min_fragment_len(rule.min_fragment_len) {
            continue;
        }
        let mut search_from = 0usize;
        while search_from + lit_chars.len() <= hay_chars.len() {
            if budget.expired() {
                return ranges;
            }
            if hay_chars[search_from..search_from + lit_chars.len()] != lit_chars[..] {
                search_from += 1;
                continue;
            }
            let norm_pos = search_from;
            let candidate: String = lit_chars.iter().collect();
            let h = xxh64(candidate.as_bytes(), 0);
            if bloom.may_contain(h) {
                if let Some((b0, b1)) = verify_normalized_match(
                    conn,
                    h,
                    &candidate,
                    norm_pos,
                    lit_chars.len(),
                    hay_block,
                    &norm,
                    hay_chunk_norm_len,
                    hay_block_start,
                    allowed_paths,
                    rule,
                ) {
                    ranges.push((b0, b1));
                }
            }
            search_from = norm_pos + 1;
        }
    }
    ranges
}

fn scan_block_bloom(
    hay_block: &str,
    hay_block_start: usize,
    rule: &FileRule,
    lens: &[usize],
    min_len: usize,
    bloom: &BloomFilter,
    db_path: &Path,
    allowed_paths: &HashSet<String>,
    budget: &ScanBudget,
) -> Vec<(usize, usize)> {
    let Ok(conn) = Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) else {
        return Vec::new();
    };
    scan_region_bloom_slice(
        hay_block,
        hay_block_start,
        lens,
        min_len,
        rule.index.scan_stride.max(1),
        bloom,
        &conn,
        allowed_paths,
        rule,
        budget,
    )
}

fn scan_region_bloom_slice(
    hay_block: &str,
    hay_block_start: usize,
    lens: &[usize],
    min_len: usize,
    _scan_stride: usize,
    bloom: &BloomFilter,
    conn: &Connection,
    allowed_paths: &HashSet<String>,
    rule: &FileRule,
    budget: &ScanBudget,
) -> Vec<(usize, usize)> {
    let norm = normalize_with_map(hay_block);
    if norm.text.chars().count() < min_len {
        return Vec::new();
    }
    let hay_chunk_norm_len = norm.text.chars().count();
    let chars: Vec<char> = norm.text.chars().collect();
    let mut byte_ranges = Vec::new();
    let mut pos = 0usize;
    while pos + min_len <= chars.len() {
        if pos > 0 && pos % 256 == 0 && budget.expired() {
            return byte_ranges;
        }
        for &len in lens {
            if pos + len > chars.len() {
                continue;
            }
            let candidate: String = chars[pos..pos + len].iter().collect();
            let h = xxh64(candidate.as_bytes(), 0);
            if !bloom.may_contain(h) {
                continue;
            }
            if let Some((b0, b1)) = verify_normalized_match(
                conn,
                h,
                &candidate,
                pos,
                len,
                hay_block,
                &norm,
                hay_chunk_norm_len,
                hay_block_start,
                allowed_paths,
                rule,
            ) {
                byte_ranges.push((b0, b1));
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
    let min_len = file_min_fragment_len(rule.min_fragment_len).max(4);
    let conn = Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let fetch = cap.saturating_mul(8).min(100_000);
    let mut stmt = conn.prepare(
        "SELECT DISTINCT path, byte_offset, byte_len FROM signatures ORDER BY sig_hash LIMIT ?1",
    )?;
    let mut rows = stmt.query(params![fetch as i64])?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let path: String = row.get(0)?;
        let offset: i64 = row.get(1)?;
        let len: i32 = row.get(2)?;
        let Some(raw) = read_literal_string(&path, offset as u64, len as u32) else {
            continue;
        };
        let norm = normalize_with_map(&raw);
        let chars: Vec<char> = norm.text.chars().collect();
        if chars.len() < min_len {
            continue;
        }
        let sample_positions = [0usize, chars.len() / 2, chars.len().saturating_sub(min_len)];
        for pos in sample_positions {
            if pos + min_len > chars.len() {
                continue;
            }
            let lit: String = chars[pos..pos + min_len].iter().collect();
            if seen.insert(lit.clone()) {
                out.push(lit);
                if out.len() >= cap {
                    return Ok(out);
                }
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

fn verify_normalized_match(
    conn: &Connection,
    hash: u64,
    candidate: &str,
    norm_pos: usize,
    norm_len: usize,
    hay_block: &str,
    hay_norm: &Normalized,
    hay_chunk_norm_len: usize,
    hay_block_start: usize,
    allowed_paths: &HashSet<String>,
    rule: &FileRule,
) -> Option<(usize, usize)> {
    let mut stmt = conn
        .prepare("SELECT path, byte_offset, byte_len FROM signatures WHERE sig_hash = ?1 LIMIT 16")
        .ok()?;
    let mut rows = stmt.query(params![hash as i64]).ok()?;
    while let Ok(Some(row)) = rows.next() {
        let path: String = row.get(0).unwrap_or_default();
        if !allowed_paths.contains(&path)
            && !allowed_paths
                .iter()
                .any(|allowed| paths_equivalent(allowed, &path))
        {
            continue;
        }
        let chunk_start: i64 = row.get(1).unwrap_or(0);
        let chunk_len: i32 = row.get(2).unwrap_or(0);
        if chunk_len <= 0 {
            continue;
        }
        let Some(raw_chunk) = read_literal_string(&path, chunk_start as u64, chunk_len as u32) else {
            continue;
        };
        let index_norm = normalize_with_map(&raw_chunk);
        if !index_norm.text.contains(candidate) {
            continue;
        }
        if !file_fragment_meets_threshold(
            norm_len,
            index_norm.text.chars().count(),
            hay_chunk_norm_len,
            rule.min_fragment_len,
            rule.min_fragment_ratio,
        ) {
            continue;
        }
        let (b0, b1) = hay_norm.orig_byte_range(hay_block, norm_pos, norm_len)?;
        return Some((hay_block_start + b0, hay_block_start + b1));
    }
    None
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

fn apply_byte_ranges(
    text: &str,
    ranges: &[(usize, usize)],
    mode: MatchMode,
    vault: Option<(&str, &crate::dlp::TokenVault)>,
) -> String {
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
        if let Some((session_id, vault)) = vault {
            let matched: String = result.chars().skip(start).take(end - start).collect();
            let token = vault.token_for(session_id, &matched);
            result = replace_char_range(&result, start, end, &token);
        } else if mode == MatchMode::Full {
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

    fn test_secret(base: &str) -> String {
        let mut s = base
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>();
        while normalize_with_map(&s).text.chars().count() < 65 {
            s.push('X');
        }
        s
    }

    fn test_cjk_secret(base: &str) -> String {
        let mut s: String = base.chars().filter(|c| !c.is_whitespace()).collect();
        while normalize_with_map(&s).text.chars().count() < 65 {
            s.push('密');
        }
        s
    }

    struct ScanFixture {
        _tmp: TempDir,
        rule: FileRule,
        snapshot: RuleSnapshot,
        allowed: HashSet<String>,
    }

    impl ScanFixture {
        fn scan(&self, hay: &str) -> String {
            scan_haystack(hay, &self.rule, &self.snapshot, &self.allowed, None)
        }
    }

    fn setup_txt_scan(file_body: &str, prefilter: bool) -> ScanFixture {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("indexed.txt");
        fs::write(&file, file_body).unwrap();
        let rule = FileRule {
            id: "scan-case".into(),
            path: tmp.path().to_path_buf(),
            enabled: true,
            recursive: true,
            trigger_window: 3,
            match_mode: MatchMode::Fragment,
            min_fragment_len: None,
            min_fragment_ratio: None,
            formats: vec!["txt".into()],
            index: FileIndexOptions {
                bloom_megabytes: 1,
                build_workers: 1,
                scan_charset_skip: prefilter,
                scan_charset_skip_threshold: 0.5,
                scan_rg_prefilter: false,
                ..Default::default()
            },
        };
        let index_root = tmp.path().join("idx");
        let snapshot = build_rule_index(&index_root, &rule).unwrap();
        let allowed: HashSet<String> = snapshot.indexed_paths.iter().cloned().collect();
        ScanFixture {
            _tmp: tmp,
            rule,
            snapshot,
            allowed,
        }
    }

    fn assert_redacted(out: &str, secret: &str, case: &str) {
        assert!(!out.contains(secret), "{case}: expected redaction");
    }

    fn assert_unchanged(out: &str, input: &str, case: &str) {
        assert_eq!(out, input, "{case}: expected unchanged passthrough");
    }

    // --- Positive: sensitive data present → must redact ---

    #[test]
    fn scan_positive_latin_paste_redacts() {
        let secret = test_secret("LATIN-LEAK-TOKEN-ALPHA");
        let fx = setup_txt_scan(&secret, true);
        let hay = format!("user asks about {secret} please help");
        let out = fx.scan(&hay);
        assert_redacted(&out, &secret, "latin paste");
    }

    #[test]
    fn scan_negative_wrapped_file_unrelated_hay_unchanged() {
        let secret = test_secret("WRAPPED-FILE-SECRET-KEY");
        let fx = setup_txt_scan(&format!("header {secret} footer"), true);
        let hay = "Public weather forecast and sports headlines.".repeat(20);
        let out = fx.scan(&hay);
        assert_unchanged(&out, &hay, "wrapped file unrelated hay");
    }

    #[test]
    fn scan_positive_cjk_paste_redacts() {
        let secret = test_cjk_secret("核心机密编号丙丁戊己");
        let fx = setup_txt_scan(&secret, true);
        let hay = format!("请分析以下内容：{secret} 谢谢");
        let out = fx.scan(&hay);
        assert_redacted(&out, &secret, "cjk paste");
    }

    #[test]
    fn scan_negative_mixed_file_unrelated_hay_unchanged() {
        let secret = test_secret("MIXED-FILE-EMBEDDED-SECRET");
        let body = format!("上海泰坦科技股份有限公司年报摘要 {secret} 仅供内部");
        let fx = setup_txt_scan(&body, true);
        let hay = "The quick brown fox jumps over the lazy dog.".repeat(25);
        let out = fx.scan(&hay);
        assert_unchanged(&out, &hay, "mixed file english hay");
    }

    #[test]
    fn scan_positive_arabic_hay_latin_secret_still_redacts() {
        let secret = test_secret("ARABIC-WRAP-SECRET-KEY");
        let fx = setup_txt_scan(&secret, true);
        let hay = format!("مرحبا model question {secret} شكرا");
        let out = fx.scan(&hay);
        assert_redacted(&out, &secret, "arabic hay latin secret");
    }

    #[test]
    fn scan_positive_with_prefilter_disabled_still_redacts() {
        let secret = test_secret("NO-PREFILTER-STILL-LEAK");
        let fx = setup_txt_scan(&secret, false);
        let hay = format!("unrelated english words then {secret}");
        let out = fx.scan(&hay);
        assert_redacted(&out, &secret, "prefilter off");
    }

    // --- Negative: no sensitive data → must pass through unchanged ---

    #[test]
    fn scan_negative_english_hay_vs_cjk_secret_file() {
        let secret = test_cjk_secret("内部资料编号甲乙丙丁");
        let fx = setup_txt_scan(&secret, true);
        let hay = "The quick brown fox jumps over the lazy dog.".repeat(30);
        let out = fx.scan(&hay);
        assert_unchanged(&out, &hay, "english hay vs cjk file");
    }

    #[test]
    fn scan_negative_cjk_hay_vs_latin_secret_file() {
        let secret = test_secret("LATIN-ONLY-SECRET-DATA");
        let fx = setup_txt_scan(&secret, true);
        let hay = "这是与索引文件无关的中文提问内容。".repeat(25);
        let out = fx.scan(&hay);
        assert_unchanged(&out, &hay, "cjk hay vs latin file");
    }

    #[test]
    fn scan_negative_latin_same_lang_unrelated() {
        let secret = test_secret("CORPORATE-SECRET-PHRASE");
        let fx = setup_txt_scan(&secret, true);
        let hay = "Please summarize public news about weather and sports today.".repeat(10);
        let out = fx.scan(&hay);
        assert_unchanged(&out, &hay, "latin unrelated");
    }

    #[test]
    fn scan_negative_arabic_hay_vs_latin_secret_file() {
        let secret = test_secret("LATIN-SECRET-FOR-ARABIC-TEST");
        let fx = setup_txt_scan(&secret, true);
        let hay = "السؤال عن الطقس والأخبار العامة اليوم ".repeat(20);
        let out = fx.scan(&hay);
        assert_unchanged(&out, &hay, "arabic hay vs latin file");
    }

    #[test]
    fn scan_negative_cyrillic_hay_vs_latin_secret_file() {
        let secret = test_secret("LATIN-SECRET-FOR-CYRILLIC-TEST");
        let fx = setup_txt_scan(&secret, true);
        let hay = "Расскажи про погоду и новости без секретов ".repeat(15);
        let out = fx.scan(&hay);
        assert_unchanged(&out, &hay, "cyrillic hay vs latin file");
    }

    #[test]
    fn scan_negative_fragment_too_short_not_redacted() {
        let secret = test_secret("SHORT-FRAGMENT-SHOULD-NOT-MATCH");
        let fx = setup_txt_scan(&secret, true);
        let short: String = secret.chars().take(40).collect();
        assert!(short.len() < 65);
        let hay = format!("partial leak {short} end");
        let out = fx.scan(&hay);
        assert_unchanged(&out, &hay, "fragment too short");
        assert!(out.contains(&short));
    }

    #[test]
    fn scan_negative_cjk_unrelated_vs_cjk_secret_file() {
        let secret = test_cjk_secret("绝密档案编号零一二三四");
        let fx = setup_txt_scan(&secret, true);
        let hay = "今天天气很好，适合出去散步和读书。".repeat(30);
        let out = fx.scan(&hay);
        assert_unchanged(&out, &hay, "cjk unrelated hay");
    }

    #[test]
    fn scan_negative_prefilter_fast_skips_unrelated_english_vs_cjk() {
        let secret = test_cjk_secret("机密文档内容不得外传");
        let fx = setup_txt_scan(&secret, true);
        let hay = "Public English question about APIs and routing.".repeat(500);
        let t0 = std::time::Instant::now();
        let out = fx.scan(&hay);
        let ms = t0.elapsed().as_millis();
        assert_unchanged(&out, &hay, "fast skip english vs cjk");
        assert!(ms < 200, "expected fast token prefilter skip, took {ms}ms");
    }

    #[test]
    fn builds_and_scans_small_dir() {
        let tmp = TempDir::new().unwrap();
        let secret = tmp.path().join("secret.txt");
        let secret_text = test_secret("TOP-SECRET-PHRASE-XYZZY");
        let mut f = File::create(&secret).unwrap();
        writeln!(f, "{secret_text}").unwrap();

        let rule = FileRule {
            id: "t1".into(),
            path: tmp.path().to_path_buf(),
            enabled: true,
            recursive: true,
            trigger_window: 3,
            match_mode: MatchMode::Fragment,
            min_fragment_len: None,
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
        let hay = format!("user pasted {secret_text} here");
        let allowed: HashSet<String> = snapshot.indexed_paths.iter().cloned().collect();
        let out = scan_haystack(&hay, &rule, &snapshot, &allowed, None);
        assert!(!out.contains(&secret_text));
        assert_eq!(out.chars().count(), hay.chars().count());
    }

    #[test]
    fn normalized_match_ignores_whitespace() {
        let tmp = TempDir::new().unwrap();
        let secret = tmp.path().join("secret.txt");
        let secret_text = test_secret("NORMALIZED-MATCH-SECRET");
        fs::write(&secret, &secret_text).unwrap();

        let rule = FileRule {
            id: "norm".into(),
            path: tmp.path().join("."),
            enabled: true,
            recursive: true,
            trigger_window: 3,
            match_mode: MatchMode::Fragment,
            min_fragment_len: None,
            min_fragment_ratio: None,
            formats: vec!["txt".into()],
            index: FileIndexOptions {
                bloom_megabytes: 1,
                build_workers: 1,
                scan_rg_prefilter: false,
                ..Default::default()
            },
        };

        let index_root = tmp.path().join("idx");
        let snapshot = build_rule_index(&index_root, &rule).unwrap();
        let spaced: String = secret_text.chars().map(|c| format!("{c} ")).collect();
        let hay = format!("payload {spaced} tail");
        let allowed: HashSet<String> = snapshot.indexed_paths.iter().cloned().collect();
        let out = scan_haystack(&hay, &rule, &snapshot, &allowed, None);
        assert!(!out.contains(&secret_text));
    }

    #[test]
    fn incremental_rebuild_skips_unchanged_files() {
        let tmp = TempDir::new().unwrap();
        let a = tmp.path().join("a.txt");
        let b = tmp.path().join("b.txt");
        fs::write(&a, test_secret("ALPHA-SECRET-ONE")).unwrap();
        fs::write(&b, test_secret("BETA-SECRET-TWO")).unwrap();

        let rule = FileRule {
            id: "inc".into(),
            path: tmp.path().to_path_buf(),
            enabled: true,
            recursive: true,
            trigger_window: 3,
            match_mode: MatchMode::Fragment,
            min_fragment_len: None,
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
        assert_eq!(snap2.generation, gen1, "unchanged files reuse prior snapshot");

        fs::write(&a, test_secret("ALPHA-SECRET-ONE-MODIFIED")).unwrap();
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

        let modified = test_secret("ALPHA-SECRET-ONE-MODIFIED");
        let hay = format!("leak {modified} end");
        let allowed: HashSet<String> = snap3.indexed_paths.iter().cloned().collect();
        let out = scan_haystack(&hay, &rule, &snap3, &allowed, None);
        assert!(!out.contains(&modified));
    }

    #[test]
    fn parallel_scan_finds_secret_in_large_haystack() {
        let tmp = TempDir::new().unwrap();
        let secret = tmp.path().join("secret.txt");
        let secret_text = test_secret("WIDE-CORPUS-LEAK-TOKEN-99");
        fs::write(&secret, &secret_text).unwrap();

        let rule = FileRule {
            id: "wide".into(),
            path: tmp.path().to_path_buf(),
            enabled: true,
            recursive: true,
            trigger_window: 3,
            match_mode: MatchMode::Fragment,
            min_fragment_len: None,
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
        let hay = format!("{padding} {secret_text} {padding}");
        let allowed: HashSet<String> = snapshot.indexed_paths.iter().cloned().collect();
        let out = scan_haystack(&hay, &rule, &snapshot, &allowed, None);
        assert!(!out.contains(&secret_text));
        assert_eq!(out.chars().count(), hay.chars().count());
    }

    #[test]
    fn scoped_scan_only_triggered_file_in_directory() {
        let tmp = TempDir::new().unwrap();
        let triggered = tmp.path().join("triggered.txt");
        let other = tmp.path().join("other.txt");
        let triggered_secret = test_secret("TRIGGERED-ONLY-SECRET-ABC");
        let other_secret = test_secret("OTHER-FILE-SECRET-XYZ");
        fs::write(&triggered, &triggered_secret).unwrap();
        fs::write(&other, &other_secret).unwrap();

        let rule = FileRule {
            id: "dir".into(),
            path: tmp.path().to_path_buf(),
            enabled: true,
            recursive: true,
            trigger_window: 3,
            match_mode: MatchMode::Fragment,
            min_fragment_len: None,
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

        let hay_triggered = format!("leak {triggered_secret} here");
        let out1 = scan_haystack(&hay_triggered, &rule, &snapshot, &allowed, None);
        assert!(!out1.contains(&triggered_secret));

        let hay_other = format!("leak {other_secret} here");
        let out2 = scan_haystack(&hay_other, &rule, &snapshot, &allowed, None);
        assert!(out2.contains(&other_secret));
    }

    #[test]
    fn charset_prefilter_skips_unrelated_haystack() {
        let tmp = TempDir::new().unwrap();
        let secret = tmp.path().join("secret.txt");
        let secret_text = test_secret("CHARSET-SKIP-SECRET-TOKEN");
        fs::write(&secret, &secret_text).unwrap();

        let rule = FileRule {
            id: "charset".into(),
            path: tmp.path().to_path_buf(),
            enabled: true,
            recursive: true,
            trigger_window: 3,
            match_mode: MatchMode::Fragment,
            min_fragment_len: None,
            min_fragment_ratio: None,
            formats: vec!["txt".into()],
            index: FileIndexOptions {
                bloom_megabytes: 1,
                build_workers: 1,
                scan_charset_skip: true,
                scan_charset_skip_threshold: 0.5,
                ..Default::default()
            },
        };

        let index_root = tmp.path().join("idx");
        let snapshot = build_rule_index(&index_root, &rule).unwrap();
        let allowed: HashSet<String> = snapshot.indexed_paths.iter().cloned().collect();

        let unrelated = "你好世界，这是与索引文件字符集几乎无关的中文内容。".repeat(20);
        let out = scan_haystack(&unrelated, &rule, &snapshot, &allowed, None);
        assert_eq!(out, unrelated);

        let hay = format!("notes {secret_text} tail");
        let out2 = scan_haystack(&hay, &rule, &snapshot, &allowed, None);
        assert!(!out2.contains(&secret_text));
    }

    /// Local `test-data/` PDFs (not in git). Run: `cargo test -p smr-core large_pdf -- --ignored --nocapture`
    fn repo_test_data_dir() -> Option<PathBuf> {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data");
        dir.is_dir().then_some(dir)
    }

    fn smallest_pdf_in(dir: &Path) -> Option<PathBuf> {
        let mut pdfs: Vec<PathBuf> = fs::read_dir(dir)
            .ok()?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
            })
            .collect();
        pdfs.sort_by_key(|p| fs::metadata(p).map(|m| m.len()).unwrap_or(u64::MAX));
        pdfs.into_iter().next()
    }

    fn large_doc_file_rule(id: &str, path: PathBuf) -> FileRule {
        FileRule {
            id: id.into(),
            path,
            enabled: true,
            recursive: false,
            trigger_window: 3,
            match_mode: MatchMode::Fragment,
            min_fragment_len: None,
            min_fragment_ratio: None,
            formats: vec!["pdf".into()],
            index: FileIndexOptions {
                bloom_megabytes: 8,
                build_workers: 4,
                scan_time_budget_ms: 1000,
                scan_charset_skip: true,
                scan_charset_skip_threshold: 0.5,
                scan_rg_prefilter: true,
                ..Default::default()
            },
        }
    }

    fn normalized_fragment_from_pdf(path: &Path, min_len: usize) -> Option<String> {
        let text = extract_text_safe(path)?;
        let norm = normalize_with_map(&text);
        let chars: Vec<char> = norm.text.chars().collect();
        if chars.len() < min_len {
            return None;
        }
        let start = chars.len() / 3;
        Some(chars[start..start + min_len].iter().collect())
    }

    #[test]
    #[ignore = "requires local test-data/*.pdf; run: cargo test -p smr-core large_pdf -- --ignored"]
    fn large_pdf_unrelated_haystack_scan_under_one_second() {
        let Some(data_dir) = repo_test_data_dir() else {
            eprintln!("skip: test-data/ not found");
            return;
        };
        let Some(pdf) = smallest_pdf_in(&data_dir) else {
            panic!("no pdf in test-data/");
        };

        let tmp = TempDir::new().unwrap();
        let index_root = tmp.path().join("idx");
        let rule = large_doc_file_rule("large-pdf", pdf.clone());
        let t0 = std::time::Instant::now();
        let snapshot = build_rule_index(&index_root, &rule).unwrap();
        let index_ms = t0.elapsed().as_millis();

        let allowed: HashSet<String> = snapshot.indexed_paths.iter().cloned().collect();
        let hay = format!(
            "{} {}",
            "The quick brown fox jumps over the lazy dog. ".repeat(40_000),
            "Unrelated ASCII payload for charset prefilter.".repeat(500)
        );

        let t1 = std::time::Instant::now();
        let out = scan_haystack(&hay, &rule, &snapshot, &allowed, None);
        let scan_ms = t1.elapsed().as_millis();

        eprintln!(
            "pdf={} size={} index_ms={} hay_bytes={} scan_ms={}",
            pdf.display(),
            fs::metadata(&pdf).map(|m| m.len()).unwrap_or(0),
            index_ms,
            hay.len(),
            scan_ms
        );

        assert_eq!(out, hay, "unrelated haystack should pass through unchanged");
        assert!(
            scan_ms < 1000,
            "scan should finish within 1s budget, took {scan_ms}ms"
        );
    }

    #[test]
    #[ignore = "requires local test-data/*.pdf; run: cargo test -p smr-core large_pdf -- --ignored"]
    fn large_pdf_detects_pasted_fragment() {
        let Some(data_dir) = repo_test_data_dir() else {
            eprintln!("skip: test-data/ not found");
            return;
        };
        let Some(pdf) = smallest_pdf_in(&data_dir) else {
            panic!("no pdf in test-data/");
        };

        let min_len = file_min_fragment_len(None);
        let Some(fragment) = normalized_fragment_from_pdf(&pdf, min_len) else {
            panic!("could not extract {}-char fragment from {}", min_len, pdf.display());
        };

        let tmp = TempDir::new().unwrap();
        let index_root = tmp.path().join("idx");
        let rule = large_doc_file_rule("large-pdf-match", pdf.clone());
        let snapshot = build_rule_index(&index_root, &rule).unwrap();
        let allowed: HashSet<String> = snapshot.indexed_paths.iter().cloned().collect();

        let hay = format!("context {fragment} tail");
        let t0 = std::time::Instant::now();
        let out = scan_haystack(&hay, &rule, &snapshot, &allowed, None);
        let scan_ms = t0.elapsed().as_millis();

        eprintln!(
            "pdf={} fragment_len={} scan_ms={} redacted={}",
            pdf.display(),
            fragment.len(),
            scan_ms,
            !out.contains(&fragment)
        );

        assert!(!out.contains(&fragment), "pasted fragment should be redacted");
        assert!(scan_ms < 1000, "scan should finish within 1s, took {scan_ms}ms");
    }

    #[test]
    #[ignore = "requires local test-data/*.pdf; run: cargo test -p smr-core large_pdf -- --ignored"]
    fn large_pdf_directory_index_scan_under_one_second() {
        let Some(data_dir) = repo_test_data_dir() else {
            eprintln!("skip: test-data/ not found");
            return;
        };

        let tmp = TempDir::new().unwrap();
        let index_root = tmp.path().join("idx");
        let rule = FileRule {
            id: "large-dir".into(),
            path: data_dir.clone(),
            enabled: true,
            recursive: true,
            trigger_window: 3,
            match_mode: MatchMode::Fragment,
            min_fragment_len: None,
            min_fragment_ratio: None,
            formats: vec!["pdf".into()],
            index: FileIndexOptions {
                bloom_megabytes: 16,
                build_workers: 4,
                scan_time_budget_ms: 1000,
                scan_charset_skip: true,
                scan_charset_skip_threshold: 0.5,
                scan_rg_prefilter: true,
                ..Default::default()
            },
        };

        let t0 = std::time::Instant::now();
        let snapshot = build_rule_index(&index_root, &rule).unwrap();
        let index_ms = t0.elapsed().as_millis();
        assert!(
            !snapshot.indexed_paths.is_empty(),
            "expected at least one indexed pdf in test-data/"
        );

        let allowed: HashSet<String> = snapshot.indexed_paths.iter().cloned().collect();
        let hay = "Hello from an unrelated English-only model prompt. ".repeat(50_000);

        let t1 = std::time::Instant::now();
        let out = scan_haystack(&hay, &rule, &snapshot, &allowed, None);
        let scan_ms = t1.elapsed().as_millis();

        eprintln!(
            "pdfs={} index_ms={} hay_bytes={} scan_ms={}",
            snapshot.indexed_paths.len(),
            index_ms,
            hay.len(),
            scan_ms
        );

        assert_eq!(out, hay);
        assert!(scan_ms < 1000, "directory scan should finish within 1s, took {scan_ms}ms");
    }
}
