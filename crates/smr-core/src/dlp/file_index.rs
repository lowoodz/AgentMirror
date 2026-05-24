use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use aho_corasick::AhoCorasick;
use anyhow::Result;
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::RwLock;

use crate::config::FileRule;
use crate::dlp::file::{load_rule_contents, FileContent};

pub const CHUNK_SIZE: usize = 8192;
pub const CHUNK_OVERLAP: usize = 64;

#[derive(Clone)]
pub struct FileChunk {
    pub source: PathBuf,
    pub chunk_index: usize,
    pub text: String,
}

#[derive(Clone)]
pub struct IndexedRule {
    pub rule: FileRule,
    pub normalized_path: String,
    pub contents: Vec<FileContent>,
    pub chunks: Vec<FileChunk>,
}

struct FileIndexState {
    rules: Vec<IndexedRule>,
    automaton: Option<AhoCorasick>,
    needle_map: Vec<(usize, usize, usize)>,
    ready: bool,
}

pub struct FileIndexManager {
    inner: Arc<RwLock<FileIndexState>>,
}

impl FileIndexManager {
    pub fn new(rules: &[FileRule]) -> Self {
        let inner = Arc::new(RwLock::new(FileIndexState {
            rules: Vec::new(),
            automaton: None,
            needle_map: Vec::new(),
            ready: false,
        }));
        let mgr = Self { inner: inner.clone() };
        let rules_vec = rules.to_vec();
        std::thread::spawn(move || {
            if let Ok(state) = build_index(&rules_vec) {
                *inner.write() = state;
            }
        });
        mgr.spawn_watcher(rules);
        mgr
    }

    pub fn is_ready(&self) -> bool {
        self.inner.read().ready
    }

    pub fn rules(&self) -> Vec<IndexedRule> {
        self.inner.read().rules.clone()
    }

    pub fn rebuild_sync(&self, rules: &[FileRule]) -> Result<()> {
        *self.inner.write() = build_index(rules)?;
        Ok(())
    }

    /// Patterns to search in haystack for a given indexed file (full text or UTF-8 chunks).
    pub fn search_patterns_for_file(&self, rule: &FileRule, file: &FileContent) -> Vec<String> {
        let state = self.inner.read();
        for indexed in &state.rules {
            if indexed.rule.path == rule.path {
                return patterns_for_file(file, &indexed.chunks);
            }
        }
        patterns_for_file(file, &[])
    }

    /// Return true if any search pattern for this file appears in haystack.
    pub fn file_content_matches(&self, haystack: &str, rule: &FileRule, file: &FileContent) -> bool {
        let patterns = self.search_patterns_for_file(rule, file);
        if patterns.is_empty() {
            return false;
        }
        let state = self.inner.read();
        if let Some(ac) = &state.automaton {
            for mat in ac.find_iter(haystack) {
                let pid = mat.pattern().as_usize();
                if let Some(&(ri, ci, _)) = state.needle_map.get(pid) {
                    if let Some(indexed) = state.rules.get(ri) {
                        if indexed.rule.path == rule.path {
                            if indexed
                                .contents
                                .get(ci)
                                .is_some_and(|c| c.source == file.source)
                            {
                                return true;
                            }
                        }
                    }
                }
            }
        }
        !super::rg::find_matching_needles(haystack, &patterns).is_empty()
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
        let rules_owned = rules.to_vec();
        std::thread::spawn(move || {
            let (tx, rx) = std::sync::mpsc::channel();
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
                std::thread::sleep(Duration::from_millis(500));
                if let Ok(state) = build_index(&rules_owned) {
                    *inner.write() = state;
                }
            }
        });
    }
}

pub fn patterns_for_file(file: &FileContent, chunks: &[FileChunk]) -> Vec<String> {
    if file.text.is_empty() {
        return Vec::new();
    }
    if file.text.chars().count() <= CHUNK_SIZE * 2 {
        return vec![file.text.clone()];
    }
    let file_chunks: Vec<String> = chunks
        .iter()
        .filter(|c| c.source == file.source)
        .map(|c| c.text.clone())
        .collect();
    if !file_chunks.is_empty() {
        return file_chunks;
    }
    utf8_safe_chunk_strings(&file.text)
}

pub fn utf8_safe_chunk_strings(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= CHUNK_SIZE * 2 {
        return vec![text.to_string()];
    }
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < chars.len() {
        let end = (start + CHUNK_SIZE).min(chars.len());
        let chunk: String = chars[start..end].iter().collect();
        if chunk.chars().count() >= 4 {
            out.push(chunk);
        }
        if end >= chars.len() {
            break;
        }
        start = end.saturating_sub(CHUNK_OVERLAP);
    }
    out
}

fn build_index(rules: &[FileRule]) -> Result<FileIndexState> {
    let mut indexed_rules = Vec::new();
    let mut patterns: Vec<String> = Vec::new();
    let mut needle_map: Vec<(usize, usize, usize)> = Vec::new();

    for (ri, rule) in rules.iter().filter(|r| r.enabled).enumerate() {
        let contents = load_rule_contents(rule)?;
        let normalized_path = rule.path.to_string_lossy().replace('\\', "/");
        let mut chunks = Vec::new();
        for (ci, file) in contents.iter().enumerate() {
            if file.text.is_empty() {
                continue;
            }
            let file_patterns = if file.text.chars().count() <= CHUNK_SIZE * 2 {
                vec![(file.text.clone(), 0usize)]
            } else {
                utf8_safe_chunk_strings(&file.text)
                    .into_iter()
                    .enumerate()
                    .map(|(idx, text)| {
                        chunks.push(FileChunk {
                            source: file.source.clone(),
                            chunk_index: idx,
                            text: text.clone(),
                        });
                        (text, idx)
                    })
                    .collect()
            };
            for (text, chunk_idx) in file_patterns {
                if text.chars().count() >= 4 {
                    patterns.push(text);
                    needle_map.push((ri, ci, chunk_idx));
                }
            }
        }
        indexed_rules.push(IndexedRule {
            rule: rule.clone(),
            normalized_path,
            contents,
            chunks,
        });
    }

    let automaton = if patterns.is_empty() {
        None
    } else {
        Some(AhoCorasick::new(&patterns)?)
    };

    Ok(FileIndexState {
        rules: indexed_rules,
        automaton,
        needle_map,
        ready: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_chunks_do_not_split_multibyte_chars() {
        let text = "前缀".repeat(5000);
        let chunks = utf8_safe_chunk_strings(&text);
        for chunk in &chunks {
            assert!(chunk.is_char_boundary(chunk.len()));
            assert!(std::str::from_utf8(chunk.as_bytes()).is_ok());
        }
        let rejoined: String = chunks.join("");
        assert!(rejoined.contains('前'));
    }
}
