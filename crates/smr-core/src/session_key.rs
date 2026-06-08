//! Stable DLP session keys derived from chat requests (clients need not send X-SMR-Session-Id).

use axum::http::HeaderMap;
use serde_json::Value;
use uuid::Uuid;

const SESSION_NS: Uuid = Uuid::NAMESPACE_OID;

/// Resolve the DLP session id for this proxy request.
///
/// Explicit `X-SMR-Session-Id` wins. Otherwise fingerprint the chat anchor
/// (system prompt + first user message + model) so multi-turn clients that replay
/// full history share one SessionGuard without client-side SMR awareness.
pub fn derive_session_id(headers: &HeaderMap, body: &[u8]) -> String {
    if let Some(explicit) = headers
        .get("x-smr-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return explicit.to_string();
    }

    if let Ok(json) = serde_json::from_slice::<Value>(body) {
        if let Some(fp) = fingerprint_from_body(&json) {
            return Uuid::new_v5(&SESSION_NS, fp.as_bytes()).to_string();
        }
    }

    Uuid::new_v4().to_string()
}

fn fingerprint_from_body(body: &Value) -> Option<String> {
    let messages = body.get("messages")?.as_array()?;
    if messages.is_empty() {
        return None;
    }

    let mut parts = Vec::new();
    if let Some(sys) = messages.first().filter(|m| role_is(m, "system")) {
        parts.push(format!("system:{}", message_content_key(sys)));
    }
    for msg in messages {
        if !role_is(msg, "user") {
            continue;
        }
        let content = message_content_key(msg);
        if content.trim().is_empty() {
            continue;
        }
        parts.push(format!("user:{content}"));
        break;
    }
    if parts.is_empty() {
        return None;
    }
    if let Some(model) = body.get("model").and_then(|m| m.as_str()) {
        parts.push(format!("model:{model}"));
    }
    Some(parts.join("\n"))
}

fn role_is(msg: &Value, role: &str) -> bool {
    msg.get("role").and_then(|r| r.as_str()) == Some(role)
}

fn message_content_key(msg: &Value) -> String {
    match msg.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};

    #[test]
    fn explicit_header_overrides_fingerprint() {
        let mut headers = HeaderMap::new();
        headers.insert("x-smr-session-id", HeaderValue::from_static("client-sess-1"));
        let body = br#"{"messages":[{"role":"user","content":"hello"}]}"#;
        assert_eq!(derive_session_id(&headers, body), "client-sess-1");
    }

    #[test]
    fn fingerprint_stable_across_growing_transcript() {
        let anchor = serde_json::json!({
            "model": "routed",
            "messages": [
                {"role": "system", "content": "You are a helpful assistant."},
                {"role": "user", "content": "list files in the project"},
            ]
        });
        let mut longer = anchor.clone();
        longer["messages"].as_array_mut().unwrap().extend([
            serde_json::json!({"role": "assistant", "content": "ok"}),
            serde_json::json!({"role": "user", "content": "read the next section"}),
        ]);

        let a = derive_session_id(&HeaderMap::new(), anchor.to_string().as_bytes());
        let b = derive_session_id(&HeaderMap::new(), longer.to_string().as_bytes());
        assert_eq!(a, b, "session key should not change when history grows");
    }

    #[test]
    fn fingerprint_changes_when_session_anchor_changes() {
        let a = derive_session_id(
            &HeaderMap::new(),
            br#"{"model":"routed","messages":[{"role":"system","content":"sys"},{"role":"user","content":"first"}]}"#,
        );
        let b = derive_session_id(
            &HeaderMap::new(),
            br#"{"model":"routed","messages":[{"role":"system","content":"sys"},{"role":"user","content":"second"}]}"#,
        );
        assert_ne!(a, b);
    }

    #[test]
    fn fingerprint_skips_leading_empty_user_messages() {
        let anchor = serde_json::json!({
            "model": "routed",
            "messages": [
                {"role": "system", "content": "sys"},
                {"role": "user", "content": ""},
                {"role": "user", "content": "real task"},
            ]
        });
        let mut longer = anchor.clone();
        longer["messages"].as_array_mut().unwrap().extend([
            serde_json::json!({"role": "assistant", "content": "ok"}),
            serde_json::json!({"role": "user", "content": "follow up"}),
        ]);

        let a = derive_session_id(&HeaderMap::new(), anchor.to_string().as_bytes());
        let b = derive_session_id(&HeaderMap::new(), longer.to_string().as_bytes());
        assert_eq!(a, b);
    }
}
