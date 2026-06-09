use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use quick_xml::events::Event;
use quick_xml::Reader;
use zip::ZipArchive;

use crate::tool_bundle::{bundled_tool_env, resolve_tool};

/// Extract readable text from supported document formats.
pub fn extract_text(path: &Path) -> Result<String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match ext.as_str() {
        "pdf" => extract_pdf(path),
        "doc" => extract_word(path),
        "docx" => extract_word(path),
        "ppt" => extract_presentation(path),
        "pptx" => extract_presentation(path),
        _ => {
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))
        }
    }
}

/// Heuristic: text plausibly came from document body (not PDF streams / embedded XML).
pub(crate) fn text_quality_ok(text: &str) -> bool {
    let t = text.trim();
    if t.len() < 16 {
        return false;
    }
    let chars: Vec<char> = t.chars().collect();
    let n = chars.len();
    let alnum = chars.iter().filter(|c| c.is_alphanumeric()).count();
    if alnum * 10 < n {
        return false;
    }
    let printable = chars
        .iter()
        .filter(|c| c.is_ascii() && (!c.is_control() || c.is_whitespace()))
        .count();
    if printable * 100 / n.max(1) < 75 {
        return false;
    }
    let lower = t.to_ascii_lowercase();
    if lower.contains("<!doctype") || (lower.contains("doctype") && lower.contains("html") && !t.contains(' ')) {
        return false;
    }
    if lower.contains("xmlns:") && (lower.contains("elsevier") || lower.contains("schemas.openxmlformats")) {
        // pdf_extract often returns publisher metadata XML, not visible page text.
        let word_like = chars
            .windows(5)
            .filter(|w| w.iter().all(|c| c.is_alphabetic()))
            .count();
        if word_like * 20 < n {
            return false;
        }
    }
    true
}

fn extract_pdf(path: &Path) -> Result<String> {
    if let Ok(text) = extract_pdf_pdftotext(path) {
        if text_quality_ok(&text) {
            return Ok(text);
        }
    }

    let bytes = std::fs::read(path)?;
    let rust_text = pdf_extract::extract_text_from_mem(&bytes)
        .with_context(|| format!("pdf_extract {}", path.display()));
    if let Ok(ref text) = rust_text {
        if text_quality_ok(text) {
            return Ok(text.clone());
        }
    }

    if let Ok(text) = extract_pdf_pdftotext(path) {
        if !text.trim().is_empty() {
            return Ok(text);
        }
    }
    if let Ok(text) = rust_text {
        if !text.trim().is_empty() {
            return Ok(text);
        }
    }

    anyhow::bail!(
        "pdf text extraction failed for {} (bundled poppler pdftotext missing or unreadable PDF)",
        path.display()
    )
}

fn extract_pdf_pdftotext(path: &Path) -> Result<String> {
    let pdftotext = resolve_tool("pdftotext");
    let mut cmd = Command::new(&pdftotext);
    cmd.args(["-enc", "UTF-8", "-nopgbrk"]).arg(path).arg("-");
    for (key, value) in bundled_tool_env(&pdftotext) {
        cmd.env(key, value);
    }
    let output = cmd.output().with_context(|| {
        format!(
            "spawn pdftotext ({})",
            pdftotext.display()
        )
    })?;
    if !output.status.success() {
        anyhow::bail!(
            "pdftotext failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn extract_word(path: &Path) -> Result<String> {
    #[cfg(target_os = "macos")]
    if let Ok(text) = extract_textutil(path) {
        if text_quality_ok(&text) {
            return Ok(text);
        }
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let text = match ext.as_str() {
        "doc" => extract_doc_legacy(path)?,
        "docx" => extract_docx_ooxml(path)?,
        _ => anyhow::bail!("unsupported word format {}", path.display()),
    };

    if text_quality_ok(&text) {
        return Ok(text);
    }

    #[cfg(target_os = "macos")]
    if let Ok(fallback) = extract_textutil(path) {
        if !fallback.trim().is_empty() {
            return Ok(fallback);
        }
    }

    Ok(text)
}

fn extract_presentation(path: &Path) -> Result<String> {
    #[cfg(target_os = "macos")]
    if let Ok(text) = extract_textutil(path) {
        if text_quality_ok(&text) {
            return Ok(text);
        }
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let text = match ext.as_str() {
        "pptx" => extract_pptx_ooxml(path)?,
        "ppt" => {
            #[cfg(not(target_os = "macos"))]
            anyhow::bail!(
                "legacy .ppt extraction requires macOS textutil for {}",
                path.display()
            );
            #[cfg(target_os = "macos")]
            extract_textutil(path)?
        }
        _ => anyhow::bail!("unsupported presentation format {}", path.display()),
    };

    if text_quality_ok(&text) {
        return Ok(text);
    }

    #[cfg(target_os = "macos")]
    if let Ok(fallback) = extract_textutil(path) {
        if !fallback.trim().is_empty() {
            return Ok(fallback);
        }
    }

    Ok(text)
}

fn extract_doc_legacy(path: &Path) -> Result<String> {
    for cmd in ["antiword", "catdoc"] {
        if let Ok(text) = extract_external_file(cmd, path) {
            if !text.trim().is_empty() {
                return Ok(text);
            }
        }
    }

    #[cfg(target_os = "macos")]
    return extract_textutil(path);

    #[cfg(not(target_os = "macos"))]
    anyhow::bail!(
        "legacy .doc extraction failed for {} (install antiword or catdoc)",
        path.display()
    )
}

#[cfg(target_os = "macos")]
fn extract_textutil(path: &Path) -> Result<String> {
    let output = Command::new("textutil")
        .args(["-convert", "txt", "-stdout"])
        .arg(path)
        .output()
        .with_context(|| format!("spawn textutil for {}", path.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "textutil failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn extract_external_file(program: &str, path: &Path) -> Result<String> {
    let output = Command::new(program)
        .arg(path)
        .output()
        .with_context(|| format!("spawn {program} for {}", path.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn extract_docx_ooxml(path: &Path) -> Result<String> {
    extract_zip_xml_entries(path, |name| {
        name.starts_with("word/") && name.ends_with(".xml") && !name.contains("_rels/")
    })
}

fn extract_pptx_ooxml(path: &Path) -> Result<String> {
    extract_zip_xml_entries(path, |name| {
        (name.starts_with("ppt/slides/slide") && name.ends_with(".xml"))
            || (name.starts_with("ppt/notesSlides/notesSlide") && name.ends_with(".xml"))
    })
}

fn extract_zip_xml_entries(path: &Path, include: impl Fn(&str) -> bool) -> Result<String> {
    let file = File::open(path)?;
    let mut archive = ZipArchive::new(file)?;
    let mut out = String::new();

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name().to_string();
        if !include(&name) {
            continue;
        }
        let mut xml = String::new();
        entry.read_to_string(&mut xml)?;
        let chunk = extract_xml_text(&xml);
        if !chunk.trim().is_empty() {
            out.push_str(&chunk);
            out.push('\n');
        }
    }

    if out.trim().is_empty() {
        anyhow::bail!("no readable text in OOXML {}", path.display());
    }
    Ok(out)
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
                    let piece = t.trim();
                    if !piece.is_empty() {
                        if !out.is_empty() && !out.ends_with(' ') {
                            out.push(' ');
                        }
                        out.push_str(piece);
                    }
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
    use std::path::PathBuf;
    use tempfile::NamedTempFile;

    fn fixture_pdf() -> Option<PathBuf> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        for rel in [
            "test-data/Aibaba, Question Directed Graph Attention Network for Numerical Reasoning over Text.pdf",
        ] {
            let path = root.join(rel);
            if path.is_file() {
                return Some(path);
            }
        }
        if let Ok(raw) = std::env::var("SMR_DOC_EXTRACT_FIXTURE_PDF") {
            let path = PathBuf::from(raw);
            if path.is_file() {
                return Some(path);
            }
        }
        None
    }

    #[test]
    fn reads_plain_text() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "hello secret").unwrap();
        let text = extract_text(f.path()).unwrap();
        assert!(text.contains("hello"));
    }

    #[test]
    fn text_quality_rejects_metadata_blob() {
        assert!(!text_quality_ok("DOCTYPEhtmlhtmlheadstyletable"));
        assert!(!text_quality_ok("nőڇ7UgWPvOsUh"));
        assert!(text_quality_ok(
            "Question Directed Graph Attention Network for Numerical Reasoning over Text Kunlong Chen"
        ));
    }

    #[test]
    fn pdf_extract_matches_pdftotext_on_fixture() {
        let Some(pdf) = fixture_pdf() else {
            eprintln!("skip pdf_extract_matches_pdftotext_on_fixture: test-data PDF missing");
            return;
        };
        let ours = extract_text(&pdf).expect("extract_text pdf");
        assert!(text_quality_ok(&ours), "extracted: {:?}", &ours[..ours.len().min(120)]);

        let external = extract_pdf_pdftotext(&pdf).expect("pdftotext");
        assert!(text_quality_ok(&external));

        let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        let ours_n = norm(&ours);
        let ext_n = norm(&external);
        assert!(
            ours_n.contains("Kunlong Chen") || ext_n.contains("Kunlong Chen"),
            "expected author name in extraction"
        );
        let overlap = ours_n
            .split(' ')
            .filter(|w| w.len() >= 5 && ext_n.contains(w))
            .count();
        assert!(
            overlap >= 8,
            "extracted text should overlap pdftotext (overlap={overlap})"
        );
    }
}
