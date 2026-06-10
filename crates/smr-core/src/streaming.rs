use bytes::Bytes;
use smr_protocol::{extract_texts, inject_response_texts, parse_json_body, serialize_json_body};

use crate::ops::OperationSecurity;

/// Scan SSE chunks and patch tool_calls when operation security triggers.
pub fn process_sse_response(
    body: &Bytes,
    ops: &OperationSecurity,
) -> anyhow::Result<(Bytes, u32, u32)> {
    let text = String::from_utf8_lossy(body);
    let mut modified = false;
    let mut new_body = String::new();
    let mut blocks = 0u32;
    let mut observes = 0u32;
    let mut saw_first_token = false;

    for line in text.lines() {
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
                let extracted = extract_texts(&json)?;
                let (replacements, b, o) = ops.process_fields_with_mode(&extracted)?;
                blocks += b;
                observes += o;
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
        new_body.push_str(line);
        new_body.push('\n');
    }

    if modified {
        Ok((Bytes::from(new_body), blocks, observes))
    } else {
        Ok((body.clone(), blocks, observes))
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
