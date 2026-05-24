use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use walkdir::WalkDir;

use crate::config::{FileRule, MatchMode};
use crate::dlp::sanitize::{sanitize_range, sanitize_whole};
use crate::dlp::session::ActiveFileContent;

#[derive(Clone)]
pub struct FileContent {
    pub source: PathBuf,
    pub text: String,
}

struct IndexedFileRule {
    rule: FileRule,
    normalized_path: String,
    contents: Vec<FileContent>,
}

pub struct FileDlp {
    rules: Vec<IndexedFileRule>,
}

impl FileDlp {
    pub fn new(rules: &[FileRule]) -> Result<Self> {
        let mut indexed = Vec::new();
        for rule in rules.iter().filter(|r| r.enabled) {
            let contents = load_rule_contents(rule)?;
            let normalized_path = normalize_path(&rule.path);
            indexed.push(IndexedFileRule {
                rule: rule.clone(),
                normalized_path,
                contents,
            });
        }
        Ok(Self { rules: indexed })
    }

    pub fn check_path_triggers(
        &self,
        session_id: &str,
        text: &str,
        activate: impl Fn(&str, &FileRule, &[FileContent]),
    ) {
        for indexed in &self.rules {
            if text.contains(&indexed.normalized_path) {
                activate(session_id, &indexed.rule, &indexed.contents);
            }
        }
    }
}

pub fn load_rule_contents(rule: &FileRule) -> Result<Vec<FileContent>> {
    let path = &rule.path;
    if !path.exists() {
        tracing::warn!(path = %path.display(), "file rule path does not exist");
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    if path.is_file() {
        if let Some(text) = read_text_file(path) {
            out.push(FileContent {
                source: path.clone(),
                text,
            });
        }
        return Ok(out);
    }

    if path.is_dir() {
        let walker = if rule.recursive {
            WalkDir::new(path).into_iter()
        } else {
            WalkDir::new(path).max_depth(1).into_iter()
        };
        for entry in walker.filter_map(|e| e.ok()) {
            let p = entry.path();
            if p.is_file() && matches_format(p, &rule.formats) {
                if let Some(text) = read_text_file(p) {
                    out.push(FileContent {
                        source: p.to_path_buf(),
                        text,
                    });
                }
            }
        }
    }
    Ok(out)
}

fn read_text_file(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))
        .map_err(|e| tracing::warn!("{e}"))
        .ok()
}

fn matches_format(path: &Path, formats: &[String]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| formats.iter().any(|f| f.eq_ignore_ascii_case(ext)))
        .unwrap_or(false)
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub fn scan_text_for_file_content(text: &str, active: &[ActiveFileContent]) -> String {
    let mut result = text.to_string();
    for item in active {
        for file in &item.contents {
            if file.text.is_empty() {
                continue;
            }
            let needles = vec![file.text.clone()];
            if !super::rg::find_matching_needles(&result, &needles).is_empty() {
                result = match item.rule.match_mode {
                    MatchMode::Full => result.replace(&file.text, &sanitize_whole(&file.text)),
                    MatchMode::Fragment => apply_fragment_matches(&result, &file.text, &item.rule),
                };
            }
        }
    }
    result
}

fn apply_fragment_matches(text: &str, needle: &str, rule: &FileRule) -> String {
    let min_len = rule.min_fragment_len.unwrap_or(12).max(4);
    if needle.chars().count() < min_len {
        return text.to_string();
    }

    let mut result = text.to_string();
    let needle_chars: Vec<char> = needle.chars().collect();
    let max_window = needle_chars.len();

    for window in 0..max_window {
        for len in min_len..=(max_window - window) {
            let fragment: String = needle_chars[window..window + len].iter().collect();
            while let Some(pos) = result.find(&fragment) {
                let char_start = result[..pos].chars().count();
                let char_end = char_start + fragment.chars().count();
                result = sanitize_range(&result, char_start, char_end);
            }
        }
    }
    result
}
