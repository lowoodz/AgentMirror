use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use quick_xml::events::Event;
use quick_xml::Reader;
use zip::ZipArchive;

/// Extract readable text from supported document formats.
pub fn extract_text(path: &Path) -> Result<String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match ext.as_str() {
        "pdf" => extract_pdf(path),
        "doc" => extract_doc(path),
        "docx" => extract_ooxml(path, "word/document.xml"),
        "pptx" => extract_pptx(path),
        _ => {
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))
        }
    }
}

fn extract_doc(path: &Path) -> Result<String> {
    #[cfg(target_os = "macos")]
    if let Ok(text) = extract_doc_textutil(path) {
        if !text.trim().is_empty() {
            return Ok(text);
        }
    }

    for cmd in ["antiword", "catdoc"] {
        if let Ok(text) = extract_doc_external(path, cmd) {
            if !text.trim().is_empty() {
                return Ok(text);
            }
        }
    }

    anyhow::bail!(
        "legacy .doc extraction failed for {} (macOS: textutil; Linux: antiword/catdoc)",
        path.display()
    )
}

#[cfg(target_os = "macos")]
fn extract_doc_textutil(path: &Path) -> Result<String> {
    let output = Command::new("textutil")
        .args(["-convert", "txt", "-stdout"])
        .arg(path)
        .output()
        .context("spawn textutil")?;
    if !output.status.success() {
        anyhow::bail!(
            "textutil failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn extract_doc_external(path: &Path, program: &str) -> Result<String> {
    let output = Command::new(program)
        .arg(path)
        .output()
        .with_context(|| format!("spawn {program}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn extract_pdf(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    pdf_extract::extract_text_from_mem(&bytes)
        .with_context(|| format!("extract pdf {}", path.display()))
}

fn extract_pptx(path: &Path) -> Result<String> {
    let file = File::open(path)?;
    let mut archive = ZipArchive::new(file)?;
    let mut out = String::new();

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name().to_string();
        if name.starts_with("ppt/slides/slide") && name.ends_with(".xml") {
            let mut xml = String::new();
            entry.read_to_string(&mut xml)?;
            out.push_str(&extract_xml_text(&xml));
            out.push('\n');
        }
    }
    Ok(out)
}

fn extract_ooxml(path: &Path, entry_name: &str) -> Result<String> {
    let file = File::open(path)?;
    let mut archive = ZipArchive::new(file)?;
    let mut entry = archive
        .by_name(entry_name)
        .with_context(|| format!("missing {entry_name} in {}", path.display()))?;
    let mut xml = String::new();
    entry.read_to_string(&mut xml)?;
    Ok(extract_xml_text(&xml))
}

fn extract_xml_text(xml: &str) -> String {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut out = String::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Text(e)) => {
                if let Ok(t) = e.unescape() {
                    out.push_str(&t);
                    out.push(' ');
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn reads_plain_text() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "hello secret").unwrap();
        let text = extract_text(f.path()).unwrap();
        assert!(text.contains("hello"));
    }
}
