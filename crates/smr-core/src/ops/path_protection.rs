use std::path::{Path, PathBuf};

use crate::config::{PathProtectionLevel, PathProtectionRule};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpIntent {
    Delete,
    Modify,
    Access,
    Unknown,
}

#[derive(Clone)]
struct CompiledPathRule {
    id: String,
    path: PathBuf,
    level: PathProtectionLevel,
}

pub struct PathProtection {
    rules: Vec<CompiledPathRule>,
}

impl PathProtection {
    pub fn new(rules: &[PathProtectionRule]) -> Self {
        let rules = rules
            .iter()
            .filter(|r| r.enabled && !r.path.as_os_str().is_empty())
            .map(|r| CompiledPathRule {
                id: r.id.clone(),
                path: r.path.clone(),
                level: r.level,
            })
            .collect();
        Self { rules }
    }

    pub fn check(&self, text: &str) -> Option<(String, PathProtectionLevel, String)> {
        if self.rules.is_empty() {
            return None;
        }

        let (paths, explicit_path, write_payload, from_exec_command) = extract_paths(text);
        if paths.is_empty() {
            return None;
        }

        let mut intent = detect_intent(text);
        if write_payload && intent == OpIntent::Unknown {
            intent = OpIntent::Modify;
        }
        if from_exec_command && intent == OpIntent::Unknown {
            intent = OpIntent::Access;
        }
        for rule in &self.rules {
            for candidate in &paths {
                if !path_matches(&rule.path, candidate) {
                    continue;
                }
                if should_block(rule.level, intent, explicit_path, from_exec_command) {
                    return Some((
                        rule.id.clone(),
                        rule.level,
                        candidate.clone(),
                    ));
                }
            }
        }
        None
    }
}

fn should_block(
    level: PathProtectionLevel,
    intent: OpIntent,
    explicit_path: bool,
    from_exec_command: bool,
) -> bool {
    match level {
        PathProtectionLevel::DenyDelete => intent == OpIntent::Delete,
        PathProtectionLevel::DenyModify => matches!(intent, OpIntent::Delete | OpIntent::Modify),
        PathProtectionLevel::DenyAccess => {
            matches!(intent, OpIntent::Delete | OpIntent::Modify | OpIntent::Access)
                || ((explicit_path || from_exec_command) && intent == OpIntent::Unknown)
        }
    }
}

fn detect_intent(text: &str) -> OpIntent {
    let lower = text.to_lowercase();

    if is_delete_intent(&lower) {
        return OpIntent::Delete;
    }
    if is_modify_intent(&lower) {
        return OpIntent::Modify;
    }
    if is_access_intent(&lower) {
        return OpIntent::Access;
    }
    OpIntent::Unknown
}

fn is_delete_intent(lower: &str) -> bool {
    lower.contains("delete_file")
        || lower.contains("remove_file")
        || lower.contains("unlink")
        || lower.contains("rmdir")
        || lower.contains("shred")
        || lower.contains(" trash ")
        || lower.contains("\"rm ")
        || lower.contains(" rm ")
        || lower.starts_with("rm ")
        || lower.contains(" rm-")
        || lower.contains("del /")
        || lower.contains("remove(")
}

fn is_modify_intent(lower: &str) -> bool {
    lower.contains("write_file")
        || lower.contains("edit_file")
        || lower.contains("search_replace")
        || lower.contains("apply_patch")
        || lower.contains("create_file")
        || lower.contains("append_file")
        || lower.contains("sed -i")
        || lower.contains("chmod ")
        || lower.contains("chown ")
        || lower.contains("chattr ")
        || lower.contains(" tee ")
        || lower.contains("\"tee ")
        || lower.contains(" mv ")
        || lower.contains("\"mv ")
        || lower.contains(" cp ")
        || lower.contains("\"cp ")
        || lower.contains("move(")
        || lower.contains("rename(")
        || shell_redirect_intent(lower)
}

/// Shell output redirect (`echo x > path`, append `>>`, cmd `echo x>file`).
fn shell_redirect_intent(lower: &str) -> bool {
    lower.contains(" >> ")
        || lower.contains(" > \"")
        || lower.contains(" > '/")
        || lower.contains(" > \"/")
        || lower.contains(" > ")
        || lower.contains(">\"")
        || lower.contains("echo ") && lower.contains('>')
}

fn is_access_intent(lower: &str) -> bool {
    lower.contains("read_file")
        || lower.contains("list_dir")
        || lower.contains("glob_file_search")
        || lower.contains("file_search")
        || lower.contains("watch(")
        || lower.contains(" cat ")
        || lower.contains("\"cat ")
        || lower.contains("grep ")
        || lower.contains("head ")
        || lower.contains("tail ")
        || lower.contains("less ")
        || lower.contains("more ")
        || lower.contains("stat ")
        || lower.contains("open(")
        || lower.contains("readdir")
        || lower.contains("scandir")
        // OpenClaw / shell exec patterns
        || lower.contains("pdftotext")
        || lower.contains("textutil")
        || lower.contains(" find ")
        || lower.contains("\"find ")
        || lower.contains(" ls ")
        || lower.contains("\"ls ")
        || lower.starts_with("ls ")
        || lower.contains(" ls-")
        || lower.contains("\"ls-")
}

fn extract_paths(text: &str) -> (Vec<String>, bool, bool, bool) {
    let mut paths = Vec::new();
    let mut explicit_path = false;
    let mut write_payload = false;
    let mut from_exec_command = false;

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        collect_paths_from_json(&value, &mut paths, &mut explicit_path);
        write_payload = has_write_payload(&value);
    }

    if let Some(cmd) = extract_command_field(text) {
        from_exec_command = true;
        for token in tokenize_command_paths(&cmd) {
            paths.push(token);
        }
    }

    if looks_like_plain_shell_command(text) {
        from_exec_command = true;
        for token in extract_plain_absolute_paths(text) {
            paths.push(token);
            explicit_path = true;
        }
    }

    paths.sort();
    paths.dedup();
    (paths, explicit_path, write_payload, from_exec_command)
}

fn looks_like_plain_shell_command(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains(" ls ")
        || lower.starts_with("ls ")
        || lower.contains(" stat ")
        || lower.starts_with("stat ")
        || lower.contains(" cat ")
        || lower.starts_with("cat ")
        || lower.contains(" rm ")
        || lower.starts_with("rm ")
        || lower.contains(" read_file")
        || lower.contains(" write_file")
}

fn extract_plain_absolute_paths(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let start = if i + 1 < bytes.len() && bytes[i] == b'/' {
            Some(i)
        } else if i + 2 < bytes.len()
            && bytes[i].is_ascii_alphabetic()
            && bytes[i + 1] == b':'
            && matches!(bytes[i + 2], b'/' | b'\\')
        {
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
                if b.is_ascii() {
                    if b.is_ascii_alphanumeric()
                        || b"/\\._-".contains(&b)
                        || drive_colon
                        || b" #+()[]@!$&',;=~".contains(&b)
                    {
                        end += 1;
                    } else {
                        break;
                    }
                } else {
                    end += 1;
                }
            }
            let slice = &text[start..end];
            if slice.len() > 2 && looks_like_path(slice.trim()) {
                out.push(slice.trim().to_string());
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

fn has_write_payload(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            map.keys().any(|k| {
                matches!(
                    k.as_str(),
                    "contents"
                        | "content"
                        | "text"
                        | "data"
                        | "body"
                        | "new_string"
                        | "replacement"
                        | "patch"
                        | "edits"
                )
            }) || map.values().any(has_write_payload)
        }
        serde_json::Value::Array(items) => items.iter().any(has_write_payload),
        _ => false,
    }
}

fn collect_paths_from_json(
    value: &serde_json::Value,
    out: &mut Vec<String>,
    explicit_path: &mut bool,
) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, val) in map {
                if is_path_key(key) {
                    if let Some(s) = val.as_str() {
                        if looks_like_path(s) {
                            out.push(s.to_string());
                            *explicit_path = true;
                        }
                    }
                }
                collect_paths_from_json(val, out, explicit_path);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_paths_from_json(item, out, explicit_path);
            }
        }
        _ => {}
    }
}

fn is_path_key(key: &str) -> bool {
    matches!(
        key,
        "path"
            | "file_path"
            | "filepath"
            | "filePath"
            | "target_path"
            | "targetPath"
            | "source"
            | "destination"
            | "dest"
            | "directory"
            | "dir"
            | "folder"
            | "filename"
            | "file"
    )
}

fn looks_like_path(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty()
        && (t.starts_with('/')
            || t.starts_with("./")
            || t.starts_with("../")
            || t.starts_with('~')
            || (t.len() > 2 && t.as_bytes()[1] == b':'))
}

fn extract_command_field(text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    value
        .get("command")
        .or_else(|| value.get("cmd"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn tokenize_command_paths(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = command.chars().peekable();
    while chars.peek().is_some() {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }
        let token = if *chars.peek().unwrap() == '"' || *chars.peek().unwrap() == '\'' {
            let quote = chars.next().unwrap();
            let mut buf = String::new();
            for c in chars.by_ref() {
                if c == quote {
                    break;
                }
                buf.push(c);
            }
            buf
        } else {
            let mut buf = String::new();
            while let Some(c) = chars.peek().copied() {
                if c.is_whitespace() {
                    break;
                }
                buf.push(c);
                chars.next();
            }
            buf
        };
        if looks_like_path(&token) {
            out.push(token);
        }
    }
    out
}

fn normalize_path(path: &str) -> String {
    let mut s = path.trim().replace('\\', "/");
    if s.starts_with("~/") {
        s = s.replacen('~', &std::env::var("HOME").unwrap_or_else(|_| "~".into()), 1);
    }
    while s.contains("//") {
        s = s.replace("//", "/");
    }
    if s.len() > 1 && s.ends_with('/') {
        s.pop();
    }
    s
}

fn path_matches(protected: &Path, candidate: &str) -> bool {
    let base = normalize_path(&protected.to_string_lossy());
    let cand = normalize_path(candidate);
    if base.is_empty() || cand.is_empty() {
        return false;
    }
    cand == base || cand.starts_with(&format!("{base}/"))
}

pub fn level_label(level: PathProtectionLevel) -> &'static str {
    match level {
        PathProtectionLevel::DenyDelete => "禁止删除",
        PathProtectionLevel::DenyModify => "禁止修改",
        PathProtectionLevel::DenyAccess => "禁止访问",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PathProtectionLevel;
    use std::path::PathBuf;

    fn rule(id: &str, path: &str, level: PathProtectionLevel) -> PathProtectionRule {
        PathProtectionRule {
            id: id.into(),
            enabled: true,
            path: PathBuf::from(path),
            level,
        }
    }

    #[test]
    fn deny_delete_blocks_rm() {
        let guard = PathProtection::new(&[rule("p1", "/etc/secrets", PathProtectionLevel::DenyDelete)]);
        let hit = guard
            .check(r#"{"command":"rm /etc/secrets/a.txt"}"#)
            .expect("blocked");
        assert_eq!(hit.0, "p1");
    }

    #[test]
    fn deny_delete_allows_read() {
        let guard = PathProtection::new(&[rule("p1", "/etc/secrets", PathProtectionLevel::DenyDelete)]);
        assert!(guard
            .check(r#"{"path":"/etc/secrets/a.txt","name":"read_file"}"#)
            .is_none());
    }

    #[test]
    fn deny_modify_blocks_write() {
        let guard = PathProtection::new(&[rule("p1", "/var/protected", PathProtectionLevel::DenyModify)]);
        assert!(guard
            .check(r#"{"path":"/var/protected/app.conf","contents":"x"}"#)
            .is_some());
    }

    #[test]
    fn deny_modify_blocks_echo_redirect() {
        let guard = PathProtection::new(&[rule("p1", "/var/protected-modify", PathProtectionLevel::DenyModify)]);
        let cmd = r#"{"command":"echo matrix-write-test > \"/var/protected-modify/marker.txt\""}"#;
        assert!(
            guard.check(cmd).is_some(),
            "shell redirect write should be modify intent"
        );
    }

    #[test]
    fn deny_access_blocks_read_file() {
        let guard = PathProtection::new(&[rule("p1", "/home/user/private", PathProtectionLevel::DenyAccess)]);
        let hit = guard
            .check(r#"{"path":"/home/user/private/note.txt"}"#)
            .expect("blocked");
        assert_eq!(hit.2, "/home/user/private/note.txt");
    }

    #[test]
    fn prefix_match_for_directory() {
        let guard = PathProtection::new(&[rule("p1", "/data/vault", PathProtectionLevel::DenyAccess)]);
        assert!(guard.check(r#"{"path":"/data/vault/sub/file.txt"}"#).is_some());
    }

    #[test]
    fn deny_access_blocks_openclaw_exec_ls() {
        let base = "/data/test-vault";
        let guard = PathProtection::new(&[rule("p1", base, PathProtectionLevel::DenyAccess)]);
        let cmd = serde_json::json!({"command": format!("ls -la \"{base}\"")}).to_string();
        assert!(
            guard.check(&cmd).is_some(),
            "exec ls on protected directory should be blocked"
        );
    }

    #[test]
    fn deny_access_blocks_openclaw_exec_pdftotext() {
        let base = "/data/test-vault";
        let pdf = format!("{base}/report-2023.pdf");
        let guard = PathProtection::new(&[rule("p1", base, PathProtectionLevel::DenyAccess)]);
        let cmd = serde_json::json!({"command": format!("pdftotext -f 1 -l 1 \"{pdf}\" -")})
            .to_string();
        assert!(
            guard.check(&cmd).is_some(),
            "exec pdftotext on protected file should be blocked"
        );
    }

    #[test]
    fn deny_access_blocks_plain_user_ls_path() {
        let base = "/data/test-vault";
        let guard = PathProtection::new(&[rule("p1", base, PathProtectionLevel::DenyAccess)]);
        let user = format!("ls {base}/report.md 查看文件大小");
        assert!(
            guard.check(&user).is_some(),
            "plain user ls on protected path should be blocked"
        );
    }

    #[test]
    fn tokenize_command_paths_handles_quoted_paths() {
        let paths = tokenize_command_paths(r#"ls -la "/data/my dir/file.txt""#);
        assert_eq!(paths, vec!["/data/my dir/file.txt"]);
    }
}
