//! Locate bundled document-parser binaries shipped with SafeRoute (poppler, etc.).

use std::path::{Path, PathBuf};

const TOOL_DIR_NAMES: &[&str] = &["doc-tools", "tools"];

/// Optional override for tests and packaging smoke checks.
pub fn tools_dir_override() -> Option<PathBuf> {
    std::env::var("SMR_TOOLS_DIR")
        .ok()
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
}

fn expand_tool_roots(base: &Path) -> Vec<PathBuf> {
    let mut out = vec![base.to_path_buf()];
    if let Ok(entries) = std::fs::read_dir(base) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.push(path);
            }
        }
    }
    out
}

/// Root directory containing bundled third-party tools, if any.
pub fn bundled_tools_dir() -> Option<PathBuf> {
    if let Some(dir) = tools_dir_override() {
        for root in expand_tool_roots(&dir) {
            if tool_present_in(&root, "pdftotext") {
                return Some(root);
            }
        }
    }

    let exe = std::env::current_exe().ok()?;
    let mut bases: Vec<PathBuf> = Vec::new();

    if let Some(parent) = exe.parent() {
        for name in TOOL_DIR_NAMES {
            bases.push(parent.join(name));
        }
        #[cfg(target_os = "macos")]
        {
            bases.push(parent.join("../Resources/doc-tools"));
            bases.push(parent.join("../Resources/tools"));
            bases.push(parent.join("resources/doc-tools"));
            bases.push(parent.join("resources/tools"));
        }
        #[cfg(target_os = "windows")]
        {
            bases.push(parent.join("resources/doc-tools"));
            bases.push(parent.join("resources/tools"));
            bases.push(parent.join("cli/doc-tools"));
        }
    }

    for base in bases {
        if let Ok(canonical) = base.canonicalize() {
            for root in expand_tool_roots(&canonical) {
                if tool_present_in(&root, "pdftotext") {
                    return Some(root);
                }
            }
        }
    }
    None
}

fn tool_present_in(root: &Path, base_name: &str) -> bool {
    resolve_tool_in(root, base_name).is_some()
}

fn exe_suffix() -> &'static str {
    #[cfg(windows)]
    {
        ".exe"
    }
    #[cfg(not(windows))]
    {
        ""
    }
}

fn tool_file_names(base_name: &str) -> [String; 2] {
    let plain = base_name.to_string();
    let with_exe = format!("{base_name}{}", exe_suffix());
    [with_exe, plain]
}

/// Resolve a bundled tool executable, or fall back to bare name for PATH lookup.
pub fn resolve_tool(base_name: &str) -> PathBuf {
    if let Some(root) = bundled_tools_dir() {
        if let Some(path) = resolve_tool_in(&root, base_name) {
            return path;
        }
    }
    PathBuf::from(format!("{base_name}{}", exe_suffix()))
}

fn resolve_tool_in(root: &Path, base_name: &str) -> Option<PathBuf> {
    let names = tool_file_names(base_name);
    for sub in ["bin", ""] {
        for name in &names {
            let path = if sub.is_empty() {
                root.join(name)
            } else {
                root.join(sub).join(name)
            };
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

/// Extra environment for bundled tool subprocesses (dynamic libraries on macOS/Windows).
pub fn bundled_tool_env(tool_path: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let tool_dir = tool_path.parent().unwrap_or_else(|| Path::new("."));
    let lib_dir = tool_dir
        .parent()
        .map(|p| p.join("lib"))
        .filter(|p| p.is_dir())
        .or_else(|| {
            let sibling = tool_dir.join("lib");
            sibling.is_dir().then_some(sibling)
        });

    #[cfg(target_os = "macos")]
    if let Some(lib) = lib_dir {
        let existing = std::env::var("DYLD_LIBRARY_PATH").unwrap_or_default();
        let merged = if existing.is_empty() {
            lib.display().to_string()
        } else {
            format!("{}:{}", lib.display(), existing)
        };
        out.push(("DYLD_LIBRARY_PATH".into(), merged));
    }

    #[cfg(windows)]
    {
        let mut path_parts = vec![tool_dir.display().to_string()];
        if let Some(lib) = lib_dir {
            path_parts.push(lib.display().to_string());
        }
        if let Ok(existing) = std::env::var("PATH") {
            path_parts.push(existing);
        }
        out.push(("PATH".into(), path_parts.join(";")));
    }

    out
}

pub fn using_bundled_tool(base_name: &str) -> bool {
    bundled_tools_dir().is_some()
        && resolve_tool_in(
            &bundled_tools_dir().unwrap_or_default(),
            base_name,
        )
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn resolves_tool_in_bundle_layout() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("doc-tools");
        fs::create_dir_all(root.join("bin")).unwrap();
        #[cfg(windows)]
        fs::write(root.join("bin/pdftotext.exe"), b"").unwrap();
        #[cfg(not(windows))]
        fs::write(root.join("bin/pdftotext"), b"").unwrap();

        std::env::set_var("SMR_TOOLS_DIR", root.to_string_lossy().as_ref());
        let path = resolve_tool("pdftotext");
        assert!(path.ends_with("pdftotext") || path.ends_with("pdftotext.exe"));
        std::env::remove_var("SMR_TOOLS_DIR");
    }

    #[test]
    fn resolves_tool_in_platform_subdir() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("doc-tools");
        let plat = root.join("darwin-arm64/bin");
        fs::create_dir_all(&plat).unwrap();
        #[cfg(windows)]
        fs::write(plat.join("pdftotext.exe"), b"").unwrap();
        #[cfg(not(windows))]
        fs::write(plat.join("pdftotext"), b"").unwrap();

        std::env::set_var("SMR_TOOLS_DIR", root.to_string_lossy().as_ref());
        let dir = bundled_tools_dir().expect("bundle dir");
        assert!(resolve_tool("pdftotext").exists());
        assert!(dir.join("bin").join("pdftotext").exists() || dir.to_string_lossy().contains("darwin-arm64"));
        std::env::remove_var("SMR_TOOLS_DIR");
    }
}
