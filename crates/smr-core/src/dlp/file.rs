use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config::FileRule;
use crate::dlp::disk_index::{FileIndexManager, filter_most_specific_indexed, IndexedRule};
use crate::dlp::session::ActiveFileContent;
use crate::dlp::shell_paths::{
    extract_json_path_fields, extract_parent_child_combinations, extract_shell_resolved_paths,
};

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

    pub fn is_index_rebuilding(&self) -> bool {
        self.index.is_rebuilding()
    }

    pub fn check_path_triggers_in_tool_text(
        &self,
        session_id: &str,
        tool_text: &str,
        activate: impl Fn(&str, &FileRule, &[String]),
    ) {
        let rules = self.index.rules();
        let mut pending: Vec<(FileRule, Vec<String>)> = Vec::new();
        for indexed in &rules {
            let candidates = extract_triggered_files(tool_text, &indexed.rule);
            let resolved = self
                .index
                .resolve_triggered_files(&indexed.rule.id, &candidates);
            if resolved.is_empty() {
                continue;
            }
            pending.push((indexed.rule.clone(), resolved));
        }
        if pending.is_empty() {
            return;
        }

        let matched: Vec<IndexedRule> = rules
            .iter()
            .filter(|ir| pending.iter().any(|(r, _)| r.id == ir.rule.id))
            .cloned()
            .collect();
        for rule in filter_most_specific_indexed(&matched) {
            let Some((_, files)) = pending.iter().find(|(r, _)| r.id == rule.id) else {
                continue;
            };
            activate(session_id, &rule, files);
        }
    }

    pub fn scan_text(
        &self,
        text: &str,
        active: &[ActiveFileContent],
        vault: Option<(&str, &crate::dlp::TokenVault)>,
    ) -> String {
        self.index.scan_and_sanitize(text, active, vault)
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
    for candidate in extract_all_path_candidates(tool_text, &rule_base) {
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

fn extract_all_path_candidates(tool_text: &str, rule_base: &str) -> Vec<String> {
    let mut out = Vec::new();
    out.extend(extract_absolute_path_candidates(tool_text));
    out.extend(extract_shell_resolved_paths(tool_text));
    out.extend(extract_json_path_fields(tool_text, rule_base));
    out.extend(extract_parent_child_combinations(tool_text, rule_base));
    out.sort();
    out.dedup();
    out
}

pub(crate) fn expand_tilde_path(path: &str) -> String {
    let trimmed = path.trim();
    if let Some(rest) = trimmed.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    trimmed.to_string()
}

fn extract_absolute_path_candidates(tool_text: &str) -> Vec<String> {
    let mut out = Vec::new();
    out.extend(extract_quoted_absolute_paths(tool_text));
    out.extend(extract_unquoted_absolute_paths(tool_text));
    out.sort();
    out.dedup();
    out
}

/// Paths inside JSON/string quotes may contain spaces, commas, etc.
fn extract_quoted_absolute_paths(tool_text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = tool_text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let quote = bytes[i];
        if quote != b'"' && quote != b'\'' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < bytes.len() {
            if bytes[j] == b'\\' {
                j += 2;
                continue;
            }
            if bytes[j] == quote {
                break;
            }
            j += 1;
        }
        if j >= bytes.len() {
            break;
        }
        let quoted = &tool_text[i + 1..j];
        if looks_like_absolute_path(quoted) {
            out.push(normalize_existing_path(quoted));
        }
        i = j + 1;
    }
    out
}

fn extract_unquoted_absolute_paths(tool_text: &str) -> Vec<String> {
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
                out.push(normalize_existing_path(&normalized));
            }
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

fn looks_like_absolute_path(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty()
        && (t.starts_with('/')
            || (t.len() > 2 && t.as_bytes()[1] == b':' && t.as_bytes()[0].is_ascii_alphabetic()))
}

fn resolve_rule_base(rule_base: &str) -> String {
    canonicalize_if_exists(rule_base)
}

fn canonicalize_if_exists(path: &str) -> String {
    let p = PathBuf::from(path);
    if p.exists() {
        if let Ok(c) = std::fs::canonicalize(&p) {
            return normalize_trigger_path(&c.to_string_lossy());
        }
    }
    normalize_trigger_path(path)
}

fn path_under_rule(path: &str, rule_base: &str) -> bool {
    let path = canonicalize_if_exists(path);
    let rule_base = resolve_rule_base(rule_base);
    #[cfg(windows)]
    {
        let path = path.to_ascii_lowercase();
        let rule_base = rule_base.to_ascii_lowercase();
        return path == rule_base || path.starts_with(&format!("{rule_base}/"));
    }
    #[cfg(not(windows))]
    {
        paths_equivalent(&path, &rule_base) || path.starts_with(&format!("{rule_base}/"))
    }
}

/// Strip Win32 verbatim `\\?\` prefix so tool paths and index paths compare equal.
pub fn strip_verbatim_path_prefix(path: &str) -> String {
    let p = normalize_path_str(path);
    p.strip_prefix("//?/").unwrap_or(&p).to_string()
}

pub fn normalize_trigger_path(path: &str) -> String {
    let mut p = strip_verbatim_path_prefix(path);
    while p.len() > 1 && p.ends_with('/') {
        p.pop();
    }
    p
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

pub(crate) fn normalize_path_str(path: &str) -> String {
    path.replace('\\', "/")
}

pub(crate) fn normalize_existing_path(path: &str) -> String {
    canonicalize_if_exists(&trim_trailing_path_escapes(path))
}

/// Shell-escaped paths in tool JSON often end with `\` before a closing quote.
fn trim_trailing_path_escapes(path: &str) -> String {
    path.trim_end_matches('\\').to_string()
}

fn is_path_char(b: u8) -> bool {
    if !b.is_ascii() {
        // UTF-8 file/dir names (e.g. CJK) are valid inside absolute paths.
        return true;
    }
    b.is_ascii_alphanumeric()
        || b"/\\._-".contains(&b)
        || b" #+()[]@!$&',;=~".contains(&b)
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
    use crate::dlp::disk_index::{filter_most_specific_rules, IndexedRule};
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn extracts_doc_via_cd_and_relative_path_in_exec() {
        let tmp = tempfile::TempDir::new().unwrap();
        let zone = tmp.path().join("protected-zone");
        fs::create_dir_all(&zone).unwrap();
        let doc = zone.join("thesis.doc");
        fs::write(&doc, b"legacy doc").unwrap();
        let zone_str = zone.to_string_lossy().replace('\\', "/");

        let rule = FileRule {
            id: "docs".into(),
            path: zone.clone(),
            enabled: true,
            recursive: true,
            trigger_window: 15,
            match_mode: MatchMode::Fragment,
            min_fragment_len: None,
            min_fragment_ratio: None,
            formats: vec!["doc".into(), "pdf".into()],
            index: FileIndexOptions::default(),
        };
        let command = format!(
            r#"cd "{zone_str}" && textutil -convert txt -output /tmp/out.txt "thesis.doc" 2>&1 && cat /tmp/out.txt | head -500"#
        );
        let tool = serde_json::json!({ "command": command }).to_string();
        let files = extract_triggered_files(&tool, &rule);
        assert_eq!(files.len(), 1, "files={files:?}");
        let expected = fs::canonicalize(&doc)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        assert!(paths_equivalent(&files[0], &expected), "got {}", files[0]);
    }

    #[test]
    fn extracts_doc_path_from_exec_command() {
        let tmp = tempfile::TempDir::new().unwrap();
        let zone = tmp.path().join("protected-zone");
        fs::create_dir_all(&zone).unwrap();
        let doc = zone.join("sample.doc");
        fs::write(&doc, b"legacy doc").unwrap();
        let doc_str = doc.to_string_lossy().replace('\\', "/");

        let rule = FileRule {
            id: "docs".into(),
            path: zone.clone(),
            enabled: true,
            recursive: true,
            trigger_window: 15,
            match_mode: MatchMode::Fragment,
            min_fragment_len: None,
            min_fragment_ratio: None,
            formats: vec!["doc".into(), "pdf".into()],
            index: FileIndexOptions::default(),
        };
        let tool = format!(
            r#"{{"command":"textutil -convert txt -stdout \"{doc_str}\" 2>&1 | head -500"}}"#
        );
        let files = extract_triggered_files(&tool, &rule);
        assert_eq!(files.len(), 1, "files={files:?}");
        let expected = fs::canonicalize(&doc)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        assert!(paths_equivalent(&files[0], &expected), "got {}", files[0]);
    }

    #[test]
    fn extracts_pdf_path_from_pdftotext_exec() {
        let tmp = tempfile::TempDir::new().unwrap();
        let zone = tmp.path().join("protected-zone");
        fs::create_dir_all(&zone).unwrap();
        let pdf = zone.join("sample.pdf");
        fs::write(&pdf, b"%PDF-1.4 stub").unwrap();

        let rule = FileRule {
            id: "pdf-zone".into(),
            path: zone.clone(),
            enabled: true,
            recursive: true,
            trigger_window: 5,
            match_mode: MatchMode::Fragment,
            min_fragment_len: Some(65),
            min_fragment_ratio: Some(0.5),
            formats: vec!["pdf".into()],
            index: FileIndexOptions::default(),
        };
        let pdf_str = pdf.to_string_lossy().replace('\\', "/");
        let tool = format!(r#"{{"command":"pdftotext \"{pdf_str}\" - 2>&1 | head -300"}}"#);
        let files = extract_triggered_files(&tool, &rule);
        assert_eq!(files.len(), 1, "files={files:?}");
        let expected = fs::canonicalize(&pdf)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        assert!(paths_equivalent(&files[0], &expected), "got {}", files[0]);
    }

    #[test]
    fn extracts_non_ascii_path_from_exec_command() {
        let tmp = tempfile::TempDir::new().unwrap();
        let zone = tmp.path().join(format!("zone-\u{6d4b}"));
        fs::create_dir_all(&zone).unwrap();
        let doc = zone.join(format!("sample-\u{6863}.doc"));
        fs::write(&doc, b"legacy doc").unwrap();
        let doc_str = doc.to_string_lossy().replace('\\', "/");

        let rule = FileRule {
            id: "unicode-zone".into(),
            path: zone.clone(),
            enabled: true,
            recursive: true,
            trigger_window: 15,
            match_mode: MatchMode::Fragment,
            min_fragment_len: None,
            min_fragment_ratio: None,
            formats: vec!["doc".into()],
            index: FileIndexOptions::default(),
        };
        let tool = format!(
            r#"{{"command":"textutil -convert txt -stdout \"{doc_str}\" 2>&1 | head -500"}}"#
        );
        let files = extract_triggered_files(&tool, &rule);
        assert_eq!(files.len(), 1, "files={files:?}");
        let expected = fs::canonicalize(&doc)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        assert!(paths_equivalent(&files[0], &expected), "got {}", files[0]);
    }

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
    fn split_dir_and_filename_json_activate_via_parent_child() {
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let zone = tmp.path().join("protected-zone");
        fs::create_dir_all(&zone).unwrap();
        let report = zone.join("report.txt");
        fs::write(&report, "P".repeat(65)).unwrap();
        let zone_str = zone.to_string_lossy().replace('\\', "/");

        let rule = FileRule {
            id: "split-fields".into(),
            path: zone.clone(),
            enabled: true,
            recursive: true,
            trigger_window: 5,
            match_mode: MatchMode::Full,
            min_fragment_len: None,
            min_fragment_ratio: None,
            formats: vec!["txt".into()],
            index: FileIndexOptions::default(),
        };
        let fdlp = FileDlp::new(std::slice::from_ref(&rule)).unwrap();
        fdlp.reload(std::slice::from_ref(&rule)).expect("index reload");
        assert!(fdlp.is_index_ready(), "file index did not become ready");

        let tool = format!(r#"{{"directory":"{zone_str}","filename":"report.txt"}}"#);
        let activated = std::sync::Mutex::new(Vec::<String>::new());
        fdlp.check_path_triggers_in_tool_text("sess", &tool, |_, _, files| {
            activated.lock().unwrap().extend(files.iter().cloned());
        });
        let resolved = activated.lock().unwrap().clone();
        assert_eq!(
            resolved.len(),
            1,
            "tool={tool:?} resolved={resolved:?}"
        );
    }

    #[test]
    fn async_index_path_trigger_resolves_probe_file() {
        use std::thread;
        use std::time::Duration;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let probe = tmp.path().join("probe.txt");
        fs::write(&probe, "P".repeat(65)).unwrap();
        let rule = FileRule {
            id: "async-probe".into(),
            path: tmp.path().to_path_buf(),
            enabled: true,
            recursive: true,
            trigger_window: 5,
            match_mode: MatchMode::Full,
            min_fragment_len: None,
            min_fragment_ratio: None,
            formats: vec!["txt".into()],
            index: FileIndexOptions::default(),
        };
        let fdlp = FileDlp::new(std::slice::from_ref(&rule)).unwrap();
        for _ in 0..300 {
            if fdlp.is_index_ready() {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        assert!(fdlp.is_index_ready(), "async file index did not become ready");

        let probe_path = probe.to_string_lossy().replace('\\', "/");
        let tool = format!(r#"{{"path":"{probe_path}"}}"#);
        let resolved = std::sync::Mutex::new(Vec::<String>::new());
        fdlp.check_path_triggers_in_tool_text("sess", &tool, |_, _, files| {
            resolved.lock().unwrap().extend(files.iter().cloned());
        });
        let resolved = resolved.lock().unwrap().clone();
        assert!(
            !resolved.is_empty(),
            "expected indexed probe path; tool={tool:?} resolved={resolved:?}"
        );
    }

    #[test]
    fn directory_only_mention_does_not_activate_index() {
        use std::thread;
        use std::time::Duration;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let probe = tmp.path().join("probe.txt");
        fs::write(&probe, "secret").unwrap();
        let rule = FileRule {
            id: "zone".into(),
            path: tmp.path().to_path_buf(),
            enabled: true,
            recursive: true,
            trigger_window: 5,
            match_mode: MatchMode::Full,
            min_fragment_len: None,
            min_fragment_ratio: None,
            formats: vec!["txt".into()],
            index: FileIndexOptions::default(),
        };
        let fdlp = FileDlp::new(std::slice::from_ref(&rule)).unwrap();
        for _ in 0..300 {
            if fdlp.is_index_ready() {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        assert!(fdlp.is_index_ready());

        let dir = tmp.path().to_string_lossy().replace('\\', "/");
        let tool = format!(r#"{{"command":"ls -la \"{dir}\""}}"#);
        let activated = std::sync::Mutex::new(false);
        fdlp.check_path_triggers_in_tool_text("sess", &tool, |_, _, _| {
            *activated.lock().unwrap() = true;
        });
        assert!(
            !*activated.lock().unwrap(),
            "directory-only tool text must not activate"
        );
    }

    #[test]
    fn extracts_macos_temp_probe_under_rule_dir() {
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let probe = tmp.path().join("probe.txt");
        fs::write(&probe, "secret").unwrap();
        let rule = FileRule {
            id: "tmp".into(),
            path: tmp.path().to_path_buf(),
            enabled: true,
            recursive: true,
            trigger_window: 3,
            match_mode: MatchMode::Full,
            min_fragment_len: None,
            min_fragment_ratio: None,
            formats: vec!["txt".into()],
            index: FileIndexOptions::default(),
        };
        let probe_path = probe.to_string_lossy().replace('\\', "/");
        let tool = format!(r#"{{"path":"{probe_path}"}}"#);
        let files = extract_triggered_files(&tool, &rule);
        assert_eq!(files.len(), 1, "tool={tool:?} files={files:?}");
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
    fn extracts_quoted_path_with_spaces_and_commas() {
        let rule = FileRule {
            id: "data".into(),
            path: PathBuf::from("/data/sample-corpus"),
            enabled: true,
            recursive: true,
            trigger_window: 3,
            match_mode: MatchMode::Fragment,
            min_fragment_len: None,
            min_fragment_ratio: None,
            formats: vec!["pdf".into()],
            index: FileIndexOptions::default(),
        };
        let tool = r#"read_file("/data/sample-corpus/NLP/Table-NLP/Aibaba, Question Directed Graph Attention Network for Numerical Reasoning over Text.pdf")"#;
        let files = extract_triggered_files(tool, &rule);
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("Numerical Reasoning over Text.pdf"));
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
