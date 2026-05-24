use std::path::{Path, PathBuf};

use anyhow::Result;
use walkdir::WalkDir;

use crate::config::{FileRule, MatchMode};
use crate::dlp::file_index::FileIndexManager;
use crate::dlp::fragment::fragment_meets_threshold;
use crate::dlp::sanitize::{sanitize_range, sanitize_whole};
use crate::dlp::session::ActiveFileContent;

#[derive(Clone)]
pub struct FileContent {
    pub source: PathBuf,
    pub text: String,
}

pub struct FileDlp {
    index: FileIndexManager,
}

impl FileDlp {
    pub fn new(rules: &[FileRule]) -> Result<Self> {
        Ok(Self {
            index: FileIndexManager::new(rules),
        })
    }

    pub fn reload(&self, rules: &[FileRule]) -> Result<()> {
        self.index.rebuild_sync(rules)
    }

    pub fn is_index_ready(&self) -> bool {
        self.index.is_ready()
    }

    pub fn check_path_triggers_in_tool_text(
        &self,
        session_id: &str,
        tool_text: &str,
        activate: impl Fn(&str, &FileRule, &[FileContent]),
    ) {
        for indexed in self.index.rules() {
            if path_trigger_match(&indexed.normalized_path, tool_text) {
                activate(session_id, &indexed.rule, &indexed.contents);
            }
        }
    }

    pub fn scan_text(&self, text: &str, active: &[ActiveFileContent]) -> String {
        let mut result = text.to_string();
        for item in active {
            for file in &item.contents {
                if file.text.is_empty() {
                    continue;
                }
                let patterns = self.index.search_patterns_for_file(&item.rule, file);
                if patterns.is_empty() || !self.index.file_content_matches(&result, &item.rule, file) {
                    continue;
                }
                result = match item.rule.match_mode {
                    MatchMode::Full => result.replace(&file.text, &sanitize_whole(&file.text)),
                    MatchMode::Fragment => {
                        apply_fragment_matches(&result, &file.text, &item.rule)
                    }
                };
            }
        }
        result
    }
}

/// Path must appear as a path segment, not as a prefix of a longer path token.
pub fn path_trigger_match(normalized_path: &str, tool_text: &str) -> bool {
    if normalized_path.is_empty() {
        return false;
    }
    tool_text.match_indices(normalized_path).any(|(pos, _)| {
        let before_ok = pos == 0 || !is_path_token_char(tool_text.as_bytes()[pos - 1]);
        let after_pos = pos + normalized_path.len();
        let after_ok = after_pos >= tool_text.len()
            || !is_path_token_char(tool_text.as_bytes()[after_pos]);
        before_ok && after_ok
    })
}

fn is_path_token_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
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
    super::doc_extract::extract_text(path)
        .map_err(|e| tracing::warn!("{e}"))
        .ok()
}

fn matches_format(path: &Path, formats: &[String]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| formats.iter().any(|f| f.eq_ignore_ascii_case(ext)))
        .unwrap_or(false)
}

fn apply_fragment_matches(text: &str, needle: &str, rule: &FileRule) -> String {
    let min_len = crate::dlp::fragment::effective_min_fragment_len(
        needle.chars().count(),
        rule.min_fragment_len,
        rule.min_fragment_ratio,
    )
    .max(4);

    if needle.chars().count() < min_len {
        return text.to_string();
    }

    let mut result = text.to_string();
    let needle_chars: Vec<char> = needle.chars().collect();
    let max_window = needle_chars.len();

    for window in 0..max_window {
        for len in min_len..=(max_window - window) {
            if !fragment_meets_threshold(
                needle.chars().count(),
                len,
                rule.min_fragment_len,
                rule.min_fragment_ratio,
            ) {
                continue;
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_trigger_avoids_prefix_false_positive() {
        assert!(!path_trigger_match("/secret", "/secrets-backup/file.txt"));
        assert!(path_trigger_match("/secret", "read /secret/file"));
    }
}
