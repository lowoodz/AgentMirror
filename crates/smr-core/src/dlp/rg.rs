use std::process::{Command, Stdio};
use std::io::Write;

use anyhow::{Context, Result};

pub fn find_matching_needles(haystack: &str, needles: &[String]) -> Vec<String> {
    if needles.is_empty() || haystack.is_empty() {
        return Vec::new();
    }
    if let Ok(found) = rg_search(haystack, needles) {
        return found;
    }
    needles
        .iter()
        .filter(|n| !n.is_empty() && haystack.contains(n.as_str()))
        .cloned()
        .collect()
}

fn rg_search(haystack: &str, needles: &[String]) -> Result<Vec<String>> {
    if !rg_available() {
        anyhow::bail!("rg not found");
    }
    let mut found = Vec::new();
    for needle in needles {
        if needle.is_empty() {
            continue;
        }
        let mut child = Command::new("rg")
            .args(["--fixed-strings", "--no-line-number", "--no-filename", "--"])
            .arg(needle)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn rg")?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(haystack.as_bytes())?;
        }
        let output = child.wait_with_output()?;
        if output.status.success() && !output.stdout.is_empty() {
            found.push(needle.clone());
        }
    }
    Ok(found)
}

fn rg_available() -> bool {
    Command::new("rg")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_substring_search() {
        let needles = vec!["secret-token".into()];
        let hay = "user pasted secret-token here";
        assert_eq!(find_matching_needles(hay, &needles), needles);
    }
}
