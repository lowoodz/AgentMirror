//! Resolve relative file paths in agent shell commands (e.g. `cd /zone && cat "file.doc"`).

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::file::{expand_tilde_path, normalize_existing_path, normalize_path_str};

const PATH_KEYS: &[&str] = &[
    "path",
    "file_path",
    "filepath",
    "filePath",
    "target_path",
    "targetPath",
    "filename",
    "file",
];

const CWD_KEYS: &[&str] = &["cwd", "working_directory", "workingDirectory", "workdir", "workDir"];

const MAX_PARENT_CANDIDATES: usize = 8;
const MAX_CHILD_CANDIDATES: usize = 16;
const MAX_PARENT_CHILD_PAIRS: usize = 64;

/// Join directory-like parent paths with relative file tokens (fallback when no cd/cwd).
pub fn extract_parent_child_combinations(tool_text: &str, rule_base: &str) -> Vec<String> {
    let parents = collect_parent_candidates(tool_text, rule_base);
    let children = collect_child_candidates(tool_text);
    if parents.is_empty() || children.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    let mut pairs = 0usize;
    'outer: for parent in parents.iter().take(MAX_PARENT_CANDIDATES) {
        for child in children.iter().take(MAX_CHILD_CANDIDATES) {
            if pairs >= MAX_PARENT_CHILD_PAIRS {
                break 'outer;
            }
            let joined = PathBuf::from(parent).join(child.as_str());
            out.push(normalize_existing_path(&joined.to_string_lossy()));
            pairs += 1;
        }
    }
    out.sort();
    out.dedup();
    out
}

fn collect_parent_candidates(tool_text: &str, rule_base: &str) -> Vec<String> {
    let mut out = Vec::new();
    if !rule_base.is_empty() {
        out.push(normalize_existing_path(rule_base));
    }
    for quoted in extract_all_quoted_strings(tool_text) {
        let expanded = expand_tilde_path(&quoted);
        if looks_like_absolute_path(&expanded) && looks_like_directory_parent(&expanded) {
            out.push(normalize_existing_path(&expanded));
        }
    }
    out.extend(extract_unquoted_absolute_directory_paths(tool_text));
    for abs in extract_unquoted_absolute_file_paths(tool_text) {
        if let Some(parent) = PathBuf::from(&abs).parent() {
            if !parent.as_os_str().is_empty() {
                out.push(normalize_existing_path(&parent.to_string_lossy()));
            }
        }
    }
    for abs in extract_quoted_absolute_file_paths(tool_text) {
        if let Some(parent) = PathBuf::from(&abs).parent() {
            if !parent.as_os_str().is_empty() {
                out.push(normalize_existing_path(&parent.to_string_lossy()));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn collect_child_candidates(tool_text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for quoted in extract_all_quoted_strings(tool_text) {
        if looks_like_absolute_path(&quoted) || looks_like_url(&quoted) {
            continue;
        }
        if looks_like_json_artifact(&quoted) {
            continue;
        }
        if looks_like_file_reference(&quoted) {
            out.push(quoted);
        }
    }
    for token in tokenize_shell_words(tool_text) {
        if looks_like_absolute_path(&token) || token.starts_with('-') {
            continue;
        }
        if looks_like_json_artifact(&token) {
            continue;
        }
        if looks_like_file_reference(&token) {
            out.push(token);
        }
    }
    out.sort();
    out.dedup();
    out
}

fn looks_like_json_artifact(s: &str) -> bool {
    s.contains('{')
        || s.contains('}')
        || s.contains(',')
        || (s.contains(':') && !looks_like_windows_drive_path(s))
}

fn looks_like_windows_drive_path(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 3
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && matches!(b[2], b'/' | b'\\')
}

fn looks_like_directory_parent(s: &str) -> bool {
    let t = s.trim().trim_end_matches('/');
    if t.is_empty() {
        return false;
    }
    Path::new(t).extension().is_none()
}

fn extract_unquoted_absolute_directory_paths(tool_text: &str) -> Vec<String> {
    extract_unquoted_absolute_paths_with_filter(tool_text, |normalized| {
        normalized.len() > 1 && looks_like_directory_parent(normalized)
    })
}

fn extract_unquoted_absolute_file_paths(tool_text: &str) -> Vec<String> {
    extract_unquoted_absolute_paths_with_filter(tool_text, |normalized| {
        normalized.len() > 2
            && normalized.contains('.')
            && Path::new(normalized)
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|ext| !ext.is_empty())
    })
}

fn extract_quoted_absolute_file_paths(tool_text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for quoted in extract_all_quoted_strings(tool_text) {
        let expanded = expand_tilde_path(&quoted);
        if !looks_like_absolute_path(&expanded) {
            continue;
        }
        if Path::new(&expanded)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| !ext.is_empty())
        {
            out.push(normalize_existing_path(&expanded));
        }
    }
    out.sort();
    out.dedup();
    out
}

fn extract_unquoted_absolute_paths_with_filter(
    tool_text: &str,
    keep: impl Fn(&str) -> bool,
) -> Vec<String> {
    let bytes = tool_text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut in_quote: Option<u8> = None;
    while i < bytes.len() {
        let b = bytes[i];
        if in_quote.is_some() {
            if b == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if Some(b) == in_quote {
                in_quote = None;
            }
            i += 1;
            continue;
        }
        if b == b'"' || b == b'\'' {
            in_quote = Some(b);
            i += 1;
            continue;
        }

        let start = if i + 2 < bytes.len()
            && bytes[i].is_ascii_alphabetic()
            && bytes[i + 1] == b':'
            && matches!(bytes[i + 2], b'/' | b'\\')
        {
            Some(i)
        } else if b == b'/' {
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
                if is_unquoted_abs_path_char(b) || drive_colon {
                    end += 1;
                } else {
                    break;
                }
            }
            let normalized = normalize_path_str(&tool_text[start..end]);
            if keep(&normalized) {
                out.push(normalize_existing_path(&normalized));
            }
            i = end;
        } else {
            i += 1;
        }
    }
    out.sort();
    out.dedup();
    out
}

fn is_unquoted_abs_path_char(b: u8) -> bool {
    if !b.is_ascii() {
        return true;
    }
    // Exclude `,` and `:` — common in JSON tool payloads, not in normal path segments.
    b.is_ascii_alphanumeric()
        || b"/\\._-".contains(&b)
        || b" #+()[]@!$&';=~".contains(&b)
}

/// Paths implied by shell `cd` / JSON `cwd` + relative file references in tool text.
pub fn extract_shell_resolved_paths(tool_text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for value in parse_json_values(tool_text) {
        let initial_cwd = cwd_from_json(&value).map(|s| PathBuf::from(normalize_existing_path(&s)));
        if let Some(cmd) = command_from_json(&value) {
            out.extend(resolve_paths_in_shell_command(&cmd, initial_cwd.as_deref()));
        }
        out.extend(json_path_fields(&value, initial_cwd.as_deref(), None));
    }
    out
}

/// Relative paths from common JSON tool fields (`path`, `file_path`, …), optionally under `rule_base`.
pub fn extract_json_path_fields(tool_text: &str, rule_base: &str) -> Vec<String> {
    let mut out = Vec::new();
    for value in parse_json_values(tool_text) {
        let initial_cwd = cwd_from_json(&value).map(|s| PathBuf::from(normalize_existing_path(&s)));
        out.extend(json_path_fields(
            &value,
            initial_cwd.as_deref(),
            Some(rule_base),
        ));
    }
    out.sort();
    out.dedup();
    out
}

fn parse_json_values(tool_text: &str) -> Vec<Value> {
    let mut out = Vec::new();
    if let Ok(v) = serde_json::from_str::<Value>(tool_text) {
        out.push(v);
        return out;
    }
    for line in tool_text.lines() {
        let line = line.trim();
        if !line.starts_with('{') {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            out.push(v);
        }
    }
    out
}

fn command_from_json(value: &Value) -> Option<String> {
    value
        .get("command")
        .or_else(|| value.get("cmd"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn cwd_from_json(value: &Value) -> Option<String> {
    for key in CWD_KEYS {
        if let Some(s) = value.get(*key).and_then(|v| v.as_str()) {
            if !s.trim().is_empty() {
                return Some(expand_tilde_path(s));
            }
        }
    }
    None
}

fn json_path_fields(
    value: &Value,
    cwd: Option<&Path>,
    rule_base: Option<&str>,
) -> Vec<String> {
    let mut out = Vec::new();
    collect_json_path_fields(value, cwd, rule_base, &mut out);
    out
}

fn collect_json_path_fields(
    value: &Value,
    cwd: Option<&Path>,
    rule_base: Option<&str>,
    out: &mut Vec<String>,
) {
    match value {
        Value::Object(map) => {
            for (key, val) in map {
                if PATH_KEYS.contains(&key.as_str()) {
                    if let Some(s) = val.as_str() {
                        if let Some(resolved) = resolve_file_reference(s, cwd, rule_base) {
                            out.push(resolved);
                        }
                    }
                }
                collect_json_path_fields(val, cwd, rule_base, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_json_path_fields(item, cwd, rule_base, out);
            }
        }
        _ => {}
    }
}

fn resolve_paths_in_shell_command(command: &str, initial_cwd: Option<&Path>) -> Vec<String> {
    let mut cwd = initial_cwd.map(|p| p.to_path_buf());
    let mut out = Vec::new();
    for statement in split_shell_statements(command) {
        let trimmed = statement.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(target) = parse_cd_target(trimmed) {
            cwd = Some(resolve_path_against(cwd.as_deref(), &target));
            continue;
        }
        if let Some(target) = parse_pushd_target(trimmed) {
            cwd = Some(resolve_path_against(cwd.as_deref(), &target));
            continue;
        }
        if let Some(cwd_ref) = cwd.as_deref() {
            out.extend(extract_file_refs_in_shell_text(trimmed, cwd_ref));
        }
    }
    out.sort();
    out.dedup();
    out
}

fn split_shell_statements(command: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut in_quote: Option<char> = None;
    let mut chars = command.chars().peekable();

    while let Some(c) = chars.next() {
        if in_quote.is_some() {
            current.push(c);
            if Some(c) == in_quote {
                in_quote = None;
            }
            continue;
        }
        if c == '"' || c == '\'' {
            in_quote = Some(c);
            current.push(c);
            continue;
        }
        if c == '&' && chars.peek() == Some(&'&') {
            chars.next();
            push_statement(&mut statements, &current);
            current.clear();
            continue;
        }
        if c == ';' {
            push_statement(&mut statements, &current);
            current.clear();
            continue;
        }
        current.push(c);
    }
    push_statement(&mut statements, &current);
    statements
}

fn push_statement(out: &mut Vec<String>, chunk: &str) {
    let trimmed = chunk.trim();
    if !trimmed.is_empty() {
        out.push(trimmed.to_string());
    }
}

fn parse_cd_target(statement: &str) -> Option<String> {
    let rest = statement.trim().strip_prefix("cd")?;
    let rest = rest.trim();
    if rest.is_empty() || rest == "-" {
        return None;
    }
    let rest = rest
        .strip_prefix("/d")
        .or_else(|| rest.strip_prefix("/D"))
        .unwrap_or(rest)
        .trim();
    if rest.starts_with('-') && !looks_like_path_token(rest) {
        return None;
    }
    extract_first_path_operand(rest)
}

fn parse_pushd_target(statement: &str) -> Option<String> {
    let rest = statement.trim().strip_prefix("pushd")?;
    let rest = rest.trim();
    if rest.is_empty() || rest == "-" {
        return None;
    }
    extract_first_path_operand(rest)
}

fn extract_first_path_operand(rest: &str) -> Option<String> {
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }
    if rest.starts_with('"') {
        return extract_quoted(rest, '"');
    }
    if rest.starts_with('\'') {
        return extract_quoted(rest, '\'');
    }
    let token = rest.split_whitespace().next()?.trim();
    if token.is_empty() || token == "-" {
        return None;
    }
    Some(token.to_string())
}

fn extract_quoted(s: &str, quote: char) -> Option<String> {
    let mut out = String::new();
    let mut escaped = false;
    for c in s.chars().skip(1) {
        if escaped {
            out.push(c);
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if c == quote {
            return Some(out);
        }
        out.push(c);
    }
    None
}

fn resolve_path_against(base: Option<&Path>, target: &str) -> PathBuf {
    let target = expand_tilde_path(target.trim().trim_matches('"').trim_matches('\''));
    let path = PathBuf::from(&target);
    if path.is_absolute() {
        return path;
    }
    if let Some(base) = base {
        return base.join(path);
    }
    path
}

fn resolve_file_reference(raw: &str, cwd: Option<&Path>, rule_base: Option<&str>) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || looks_like_url(raw) {
        return None;
    }
    if looks_like_absolute_path(raw) {
        return Some(normalize_existing_path(raw));
    }
    if let Some(cwd) = cwd {
        let joined = cwd.join(raw);
        return Some(normalize_existing_path(&joined.to_string_lossy()));
    }
    if let Some(rule_base) = rule_base {
        let joined = PathBuf::from(rule_base).join(raw);
        let joined_str = normalize_existing_path(&joined.to_string_lossy());
        if path_starts_with_rule(&joined_str, rule_base) {
            return Some(joined_str);
        }
    }
    None
}

fn path_starts_with_rule(path: &str, rule_base: &str) -> bool {
    let path = normalize_path_str(path);
    let rule = normalize_existing_path(rule_base);
    path == rule || path.starts_with(&format!("{rule}/"))
}

fn extract_file_refs_in_shell_text(text: &str, cwd: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for quoted in extract_all_quoted_strings(text) {
        if looks_like_absolute_path(&quoted) {
            continue;
        }
        if looks_like_file_reference(&quoted) {
            out.push(normalize_existing_path(
                &cwd.join(&quoted).to_string_lossy(),
            ));
        }
    }
    for token in tokenize_shell_words(text) {
        if looks_like_absolute_path(&token) {
            continue;
        }
        if looks_like_file_reference(&token) {
            out.push(normalize_existing_path(&cwd.join(&token).to_string_lossy()));
        }
    }
    out
}

fn extract_all_quoted_strings(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let quote = bytes[i];
        if quote != b'"' && quote != b'\'' {
            i += 1;
            continue;
        }
        if let Some(s) = extract_quoted(&text[i..], quote as char) {
            out.push(s);
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
        i = if j < bytes.len() { j + 1 } else { bytes.len() };
    }
    out
}

fn tokenize_shell_words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_quote: Option<char> = None;
    for c in text.chars() {
        if in_quote.is_some() {
            if Some(c) == in_quote {
                in_quote = None;
            } else {
                current.push(c);
            }
            continue;
        }
        if c == '"' || c == '\'' {
            in_quote = Some(c);
            continue;
        }
        if c.is_whitespace() || c == '|' {
            push_word(&mut out, &current);
            current.clear();
            continue;
        }
        current.push(c);
    }
    push_word(&mut out, &current);
    out
}

fn push_word(out: &mut Vec<String>, word: &str) {
    let word = word.trim();
    if word.is_empty() || word.starts_with('-') || is_shell_noise(word) {
        return;
    }
    out.push(word.to_string());
}

fn is_shell_noise(word: &str) -> bool {
    matches!(
        word,
        "2>&1"
            | "/dev/null"
            | ">/dev/null"
            | "2>/dev/null"
            | "stdout"
            | "stderr"
            | "txt"
            | "text"
            | "pdf"
            | "doc"
            | "docx"
    ) || word.parse::<f64>().is_ok()
}

fn looks_like_absolute_path(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty()
        && (t.starts_with('/')
            || t.starts_with("~/")
            || (t.len() > 2 && t.as_bytes()[1] == b':' && t.as_bytes()[0].is_ascii_alphabetic()))
}

fn looks_like_path_token(s: &str) -> bool {
    s.contains('/') || s.contains('\\') || s.contains(':')
}

fn looks_like_file_reference(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() || t == "." || t == ".." {
        return false;
    }
    if t.contains("://") {
        return false;
    }
    Path::new(t)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| !ext.is_empty() && ext.len() <= 12)
}

fn looks_like_url(s: &str) -> bool {
    s.contains("://")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn cd_then_quoted_relative_resolves_under_cwd() {
        let tmp = TempDir::new().unwrap();
        let zone = tmp.path().join("protected-zone");
        fs::create_dir_all(&zone).unwrap();
        let doc = zone.join("thesis.doc");
        fs::write(&doc, b"content").unwrap();
        let zone_str = zone.to_string_lossy().replace('\\', "/");

        let command = format!(
            r#"cd "{zone_str}" && textutil -convert txt -output /tmp/out.txt "thesis.doc" 2>&1 && cat /tmp/out.txt | head -500"#
        );
        let tool = serde_json::json!({ "command": command }).to_string();
        let paths = extract_shell_resolved_paths(&tool);
        assert_eq!(paths.len(), 1, "paths={paths:?}");
        let expected = fs::canonicalize(&doc)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        assert_eq!(normalize_path_str(&paths[0]), normalize_path_str(&expected));
    }

    #[test]
    fn json_cwd_with_relative_path_field() {
        let tmp = TempDir::new().unwrap();
        let zone = tmp.path().join("zone");
        fs::create_dir_all(&zone).unwrap();
        let file = zone.join("report.txt");
        fs::write(&file, b"x").unwrap();
        let zone_str = zone.to_string_lossy().replace('\\', "/");

        let tool = format!(
            r#"{{"cwd":"{zone_str}","path":"report.txt"}}"#
        );
        let paths = extract_json_path_fields(&tool, &zone_str);
        assert_eq!(paths.len(), 1);
        let expected = fs::canonicalize(&file)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        assert_eq!(normalize_path_str(&paths[0]), normalize_path_str(&expected));
    }

    #[test]
    fn relative_json_path_under_rule_base() {
        let rule = "/data/protected";
        let tool = r#"{"path":"nested/report.pdf"}"#;
        let paths = extract_json_path_fields(tool, rule);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], "/data/protected/nested/report.pdf");
    }

    #[test]
    fn cd_then_non_ascii_relative_filename() {
        let tmp = TempDir::new().unwrap();
        let zone = tmp.path().join(format!("zone-\u{7f16}\u{8bd1}"));
        fs::create_dir_all(&zone).unwrap();
        let doc = zone.join("\u{6bd5}\u{4e1a}\u{8bba}\u{6587}.doc");
        fs::write(&doc, b"content").unwrap();
        let zone_str = zone.to_string_lossy().replace('\\', "/");
        let doc_name = "\u{6bd5}\u{4e1a}\u{8bba}\u{6587}.doc";

        let command = format!(
            r#"cd "{zone_str}" && textutil -convert txt -output /tmp/out.txt "{doc_name}" 2>&1 && cat /tmp/out.txt | head -500"#
        );
        let tool = serde_json::json!({ "command": command }).to_string();
        let paths = extract_shell_resolved_paths(&tool);
        assert_eq!(paths.len(), 1, "paths={paths:?}");
        let expected = fs::canonicalize(&doc)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        assert_eq!(normalize_path_str(&paths[0]), normalize_path_str(&expected));
    }

    #[test]
    fn ls_directory_does_not_emit_relative_file_paths() {
        let tool = r#"{"command":"ls -la \"/data/protected\""}"#;
        assert!(extract_shell_resolved_paths(tool).is_empty());
    }

    #[test]
    fn parent_child_combines_split_json_fields() {
        let tmp = TempDir::new().unwrap();
        let zone = tmp.path().join("protected-zone");
        fs::create_dir_all(&zone).unwrap();
        let report = zone.join("report.pdf");
        fs::write(&report, b"%PDF").unwrap();
        let zone_str = zone.to_string_lossy().replace('\\', "/");

        let tool = format!(r#"{{"directory":"{zone_str}","filename":"report.pdf"}}"#);
        let paths = extract_parent_child_combinations(&tool, &zone_str);
        assert_eq!(paths.len(), 1, "paths={paths:?}");
        let expected = fs::canonicalize(&report)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        assert_eq!(normalize_path_str(&paths[0]), normalize_path_str(&expected));
    }

    #[test]
    fn parent_child_nested_relative_file() {
        let rule = "/data/protected";
        let tool = r#"{"root":"/data/protected","target":"nested/report.pdf"}"#;
        let paths = extract_parent_child_combinations(tool, rule);
        assert_eq!(paths.len(), 1, "paths={paths:?}");
        assert_eq!(paths[0], "/data/protected/nested/report.pdf");
    }

    #[test]
    fn unquoted_scan_skips_slashes_inside_json_quotes() {
        let tool = r#"{"target":"nested/report.pdf"}"#;
        assert!(extract_unquoted_absolute_file_paths(tool).is_empty());
    }

    #[test]
    fn parent_child_directory_only_has_no_children() {
        let tool = r#"{"command":"ls -la \"/data/protected\""}"#;
        assert!(extract_parent_child_combinations(tool, "/data/protected").is_empty());
    }

    #[test]
    fn parent_child_respects_pair_limit() {
        let mut parents = Vec::new();
        for i in 0..20 {
            parents.push(format!("/data/zone{i}"));
        }
        let mut children = Vec::new();
        for i in 0..20 {
            children.push(format!("file{i}.txt"));
        }
        let tool = format!(
            "{} {}",
            parents
                .iter()
                .map(|p| format!("\"{p}\""))
                .collect::<Vec<_>>()
                .join(" "),
            children
                .iter()
                .map(|c| format!("\"{c}\""))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let paths = extract_parent_child_combinations(&tool, "");
        assert!(paths.len() <= MAX_PARENT_CHILD_PAIRS, "len={}", paths.len());
    }
}
