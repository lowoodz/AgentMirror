use bytes::Bytes;
use smr_protocol::{extract_texts, inject_response_texts, parse_json_body, serialize_json_body};

use crate::ops::OperationSecurity;
use crate::sse_tool_ops::transform_buffered_sse_ops;

/// Scan SSE chunks: DLP (response-side file/content redaction) and operation security.
pub fn process_sse_response(
    body: &Bytes,
    session_id: &str,
    dlp: Option<&crate::dlp::DlpEngine>,
    ops: Option<&OperationSecurity>,
) -> anyhow::Result<(Bytes, u32, u32, u32)> {
    let mut text = body.to_vec();
    let mut blocks = 0u32;
    let mut observes = 0u32;
    let mut dlp_count = 0u32;
    let mut modified = false;

    if ops.is_some() {
        let body_str = String::from_utf8_lossy(&text);
        let (transformed, gate_blocks) = transform_buffered_sse_ops(&body_str, ops);
        if gate_blocks > 0 {
            modified = true;
            blocks += gate_blocks;
        }
        if transformed != body_str {
            modified = true;
            text = transformed.into_bytes();
        }
    }

    let body_str = String::from_utf8_lossy(&text);
    let mut new_body = String::new();
    let mut saw_first_token = false;

    for line in body_str.lines() {
        if let Some(data) = line.strip_prefix("data: ") {
            if data.trim() == "[DONE]" {
                new_body.push_str(line);
                new_body.push('\n');
                continue;
            }
            if let Ok(mut json) = parse_json_body(data.as_bytes()) {
                if !saw_first_token && crate::router::sse_has_first_token(data.as_bytes()) {
                    saw_first_token = true;
                }

                if let Some(dlp) = dlp {
                    let extracted = extract_texts(&json)?;
                    let (replacements, count) =
                        dlp.process_response(session_id, &json, &extracted)?;
                    dlp_count += count as u32;
                    if !replacements.is_empty() {
                        inject_response_texts(&mut json, &replacements)?;
                        let patched = String::from_utf8(serialize_json_body(&json)?)?;
                        new_body.push_str("data: ");
                        new_body.push_str(&patched);
                        new_body.push('\n');
                        modified = true;
                        continue;
                    }
                }
            }
        }
        new_body.push_str(line);
        new_body.push('\n');
    }

    if modified {
        Ok((Bytes::from(new_body), blocks, observes, dlp_count))
    } else {
        Ok((body.clone(), blocks, observes, dlp_count))
    }
}

pub fn is_sse_content_type(headers: &http::HeaderMap) -> bool {
    headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("text/event-stream"))
        .unwrap_or(false)
}

pub fn request_wants_stream(body: &[u8]) -> bool {
    parse_json_body(body)
        .ok()
        .and_then(|j| j.get("stream").and_then(|s| s.as_bool()))
        .unwrap_or(false)
}
