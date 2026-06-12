//! User-facing path strings for config / admin UI (strip Win32 verbatim prefixes, map UNC → drive).

use std::path::{Path, PathBuf};

/// Strip Win32 extended `\\?\` / `\\?\UNC\` prefix (normalized to forward slashes).
pub fn strip_verbatim_path_prefix(path: &str) -> String {
    let mut p = path.replace('\\', "/");
    if let Some(rest) = p.strip_prefix("//?/UNC/") {
        p = format!("UNC/{rest}");
    } else if let Some(rest) = p.strip_prefix("//?/") {
        p = rest.to_string();
    }
    p
}

/// Path suitable for smr.yaml and admin UI after drag-drop or paste.
pub fn display_path_for_config(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let path = PathBuf::from(trimmed);
    if path.exists() {
        if let Ok(canon) = path.canonicalize() {
            if let Some(display) = user_facing_from_canonical(&canon) {
                return display;
            }
        }
    }

    format_display_path(trimmed)
}

#[cfg(windows)]
fn user_facing_from_canonical(canon: &Path) -> Option<String> {
    for drive in b'A'..=b'Z' {
        let letter = drive as char;
        let root = PathBuf::from(format!("{letter}:\\"));
        if !root.exists() {
            continue;
        }
        let Ok(root_canon) = root.canonicalize() else {
            continue;
        };
        if canon.starts_with(&root_canon) {
            let rel = canon.strip_prefix(&root_canon).unwrap_or(canon);
            let rel = rel.to_string_lossy();
            let rel = rel.trim_start_matches(['\\', '/']);
            if rel.is_empty() {
                return Some(format!("{letter}:\\"));
            }
            return Some(format!("{letter}:\\{}", rel.replace('/', "\\")));
        }
    }
    None
}

#[cfg(not(windows))]
fn user_facing_from_canonical(canon: &Path) -> Option<String> {
    Some(format_display_path(&canon.to_string_lossy()))
}

fn format_display_path(path: &str) -> String {
    let stripped = strip_verbatim_path_prefix(path);
    #[cfg(windows)]
    {
        stripped.replace('/', "\\")
    }
    #[cfg(not(windows))]
    {
        stripped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_verbatim_drive_prefix() {
        assert_eq!(
            strip_verbatim_path_prefix(r"\\?\C:\Users\Public\smr-staging"),
            "C:/Users/Public/smr-staging"
        );
    }

    #[test]
    fn strips_verbatim_unc_prefix() {
        assert_eq!(
            strip_verbatim_path_prefix(
                r"\\?\UNC\localhost@9843\DavWWWRoot\AI-Projects\SecureModelRoute\config"
            ),
            "UNC/localhost@9843/DavWWWRoot/AI-Projects/SecureModelRoute/config"
        );
    }

    #[test]
    fn display_nonexistent_strips_verbatim() {
        let out = display_path_for_config(r"\\?\C:\no-such-dir-xyz\abc");
        assert!(!out.starts_with(r"\\?\"));
        assert!(out.contains("no-such-dir-xyz"));
    }

    #[cfg(windows)]
    #[test]
    fn display_existing_maps_to_drive_letter() {
        let temp = std::env::temp_dir();
        if !temp.exists() {
            return;
        }
        let display = display_path_for_config(&temp.to_string_lossy());
        assert!(
            display.contains(':'),
            "expected drive letter path, got {display}"
        );
        assert!(
            !display.starts_with(r"\\?\"),
            "verbatim prefix should be stripped: {display}"
        );
    }
}
