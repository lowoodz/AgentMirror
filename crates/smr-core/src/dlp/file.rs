use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config::FileRule;
use crate::dlp::disk_index::{FileIndexManager, filter_most_specific_rules};
use crate::dlp::session::ActiveFileContent;

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
        activate: impl Fn(&str, &FileRule, &[String]),
    ) {
        let rules = self.index.rules();
        for rule in filter_most_specific_rules(&rules, tool_text) {
            let candidates = extract_triggered_files(tool_text, &rule);
            let resolved = self.index.resolve_triggered_files(&rule.id, &candidates);
            if resolved.is_empty() {
                continue;
            }
            activate(session_id, &rule, &resolved);
        }
    }

    pub fn scan_text(&self, text: &str, active: &[ActiveFileContent]) -> String {
        self.index.scan_and_sanitize(text, active)
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

/// Extract concrete file paths from tool text that fall under `rule.path`.
pub fn extract_triggered_files(tool_text: &str, rule: &FileRule) -> Vec<String> {
    let rule_base = normalize_path_str(&rule.path.to_string_lossy());
    if rule.path.is_file() {
        if path_trigger_match(&rule_base, tool_text) {
            return vec![rule_base];
        }
        return Vec::new();
    }

    let mut out = HashSet::new();
    for candidate in extract_absolute_path_candidates(tool_text) {
        if !path_under_rule(&candidate, &rule_base) {
            continue;
        }
        if !matches_format(Path::new(&candidate), &rule.formats) {
            continue;
        }
        out.insert(normalize_existing_path(&candidate));
    }
    out.into_iter().collect()
}

fn extract_absolute_path_candidates(tool_text: &str) -> Vec<String> {
    let bytes = tool_text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let start = if i + 2 < bytes.len()
            && bytes[i].is_ascii_alphabetic()
            && bytes[i + 1] == b':'
            && matches!(bytes[i + 2], b'/' | b'\\')
        {
            Some(i)
        } else if bytes[i] == b'/' {
            Some(i)
        } else {
            None
        };

        if let Some(start) = start {
            let mut end = start;
            while end < bytes.len() {
                let b = bytes[end];
                let drive_colon =
                    end == start + 1 && b == b':' && bytes[start].is_ascii_alphabetic();
                if is_path_char(b) || drive_colon {
                    end += 1;
                } else {
                    break;
                }
            }
            let normalized = normalize_path_str(&tool_text[start..end]);
            if normalized.len() > 2 && normalized.contains('.') {
                out.push(normalized);
            }
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

fn path_under_rule(path: &str, rule_base: &str) -> bool {
    let path = normalize_trigger_path(path);
    let rule_base = normalize_trigger_path(rule_base);
    #[cfg(windows)]
    {
        let path = path.to_ascii_lowercase();
        let rule_base = rule_base.to_ascii_lowercase();
        return path == rule_base || path.starts_with(&format!("{rule_base}/"));
    }
    #[cfg(not(windows))]
    {
        path == rule_base || path.starts_with(&format!("{rule_base}/"))
    }
}

/// Strip Win32 verbatim `\\?\` prefix so tool paths and index paths compare equal.
pub fn strip_verbatim_path_prefix(path: &str) -> String {
    let p = normalize_path_str(path);
    p.strip_prefix("//?/").unwrap_or(&p).to_string()
}

pub fn normalize_trigger_path(path: &str) -> String {
    strip_verbatim_path_prefix(path)
}

pub fn paths_equivalent(a: &str, b: &str) -> bool {
    let a = normalize_trigger_path(a);
    let b = normalize_trigger_path(b);
    #[cfg(windows)]
    {
        a.eq_ignore_ascii_case(&b)
    }
    #[cfg(not(windows))]
    {
        a == b
    }
}

pub fn path_basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn normalize_path_str(path: &str) -> String {
    path.replace('\\', "/")
}

fn normalize_existing_path(path: &str) -> String {
    let p = PathBuf::from(path);
    if p.is_file() {
        if let Ok(canon) = std::fs::canonicalize(&p) {
            return normalize_trigger_path(&canon.to_string_lossy());
        }
    }
    normalize_trigger_path(path)
}

fn is_path_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"/\\._-".contains(&b)
}

fn is_path_token_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

fn matches_format(path: &Path, formats: &[String]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| formats.iter().any(|f| f.eq_ignore_ascii_case(ext)))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FileIndexOptions, FileRule, MatchMode};
    use crate::dlp::disk_index::IndexedRule;
    use std::path::PathBuf;

    #[test]
    fn path_trigger_avoids_prefix_false_positive() {
        assert!(!path_trigger_match("/secret", "/secrets-backup/file.txt"));
        assert!(path_trigger_match("/secret", "read /secret/file"));
    }

    #[test]
    fn extracts_windows_drive_file_under_rule_dir() {
        let rule = FileRule {
            id: "secrets".into(),
            path: PathBuf::from(r"C:\Users\Public\smr-app-test-secrets"),
            enabled: true,
            recursive: false,
            trigger_window: 2,
            match_mode: MatchMode::Full,
            min_fragment_len: None,
            min_fragment_ratio: None,
            formats: vec!["txt".into()],
            index: FileIndexOptions::default(),
        };
        let tool = r#"{"path": "C:/Users/Public/smr-app-test-secrets/project.txt"}"#;
        let files = extract_triggered_files(tool, &rule);
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("/smr-app-test-secrets/project.txt"));
    }

    #[test]
    fn extracts_specific_file_under_rule_dir() {
        let rule = FileRule {
            id: "docs".into(),
            path: PathBuf::from("/data/documents"),
            enabled: true,
            recursive: true,
            trigger_window: 3,
            match_mode: MatchMode::Fragment,
            min_fragment_len: None,
            min_fragment_ratio: None,
            formats: vec!["txt".into(), "py".into()],
            index: FileIndexOptions::default(),
        };
        let tool = r#"read_file("/data/documents/projects/ml_test.py")"#;
        let files = extract_triggered_files(tool, &rule);
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("/projects/ml_test.py"));
    }

    #[test]
    fn ignores_directory_only_mention() {
        let rule = FileRule {
            id: "docs".into(),
            path: PathBuf::from("/data/documents"),
            enabled: true,
            recursive: true,
            trigger_window: 3,
            match_mode: MatchMode::Fragment,
            min_fragment_len: None,
            min_fragment_ratio: None,
            formats: vec!["txt".into()],
            index: FileIndexOptions::default(),
        };
        let tool = r#"{"path": "/data/documents"}"#;
        assert!(extract_triggered_files(tool, &rule).is_empty());
    }

    #[test]
    fn most_specific_path_wins_over_parent() {
        let parent = IndexedRule {
            rule: FileRule {
                id: "parent".into(),
                path: PathBuf::from("/data/parent"),
                enabled: true,
                recursive: true,
                trigger_window: 3,
                match_mode: MatchMode::Fragment,
                min_fragment_len: None,
                min_fragment_ratio: None,
                formats: vec!["txt".into()],
                index: FileIndexOptions::default(),
            },
            normalized_path: "/data/parent".into(),
        };
        let child = IndexedRule {
            rule: FileRule {
                id: "child".into(),
                path: PathBuf::from("/data/parent/child"),
                enabled: true,
                recursive: true,
                trigger_window: 3,
                match_mode: MatchMode::Fragment,
                min_fragment_len: None,
                min_fragment_ratio: None,
                formats: vec!["txt".into()],
                index: FileIndexOptions::default(),
            },
            normalized_path: "/data/parent/child".into(),
        };
        let rules = vec![parent, child];
        let tool = "read_file /data/parent/child/report.txt";
        let matched = filter_most_specific_rules(&rules, tool);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].id, "child");
    }

    #[test]
    fn parent_still_triggers_when_child_not_mentioned() {
        let parent = IndexedRule {
            rule: FileRule {
                id: "parent".into(),
                path: PathBuf::from("/data/parent"),
                enabled: true,
                recursive: true,
                trigger_window: 3,
                match_mode: MatchMode::Fragment,
                min_fragment_len: None,
                min_fragment_ratio: None,
                formats: vec!["txt".into()],
                index: FileIndexOptions::default(),
            },
            normalized_path: "/data/parent".into(),
        };
        let child = IndexedRule {
            rule: FileRule {
                id: "child".into(),
                path: PathBuf::from("/data/parent/child"),
                enabled: true,
                recursive: true,
                trigger_window: 3,
                match_mode: MatchMode::Fragment,
                min_fragment_len: None,
                min_fragment_ratio: None,
                formats: vec!["txt".into()],
                index: FileIndexOptions::default(),
            },
            normalized_path: "/data/parent/child".into(),
        };
        let rules = vec![parent, child];
        let tool = "read_file /data/parent/top.txt";
        let matched = filter_most_specific_rules(&rules, tool);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].id, "parent");
    }
}
