//! Parse traffic snapshot bodies for the admin API (JSON, SSE, raw text).

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrafficContentKind {
    Json,
    Sse,
    Raw,
}

impl TrafficContentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Sse => "sse",
            Self::Raw => "raw",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SseEventView {
    pub index: usize,
    pub raw: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parsed: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ParsedTrafficBody {
    pub kind: TrafficContentKind,
    pub parsed: Option<Value>,
    pub sse_events: Vec<SseEventView>,
    /// Display/copy text with `data: [DONE]` lines removed.
    pub text_clean: String,
}

pub fn parse_traffic_body(data: &[u8]) -> ParsedTrafficBody {
    let text = String::from_utf8_lossy(data).into_owned();

    if let Ok(v) = serde_json::from_slice::<Value>(data) {
        return ParsedTrafficBody {
            kind: TrafficContentKind::Json,
            parsed: Some(v),
            sse_events: Vec::new(),
            text_clean: text,
        };
    }

    if looks_like_sse(&text) {
        let (events, clean) = parse_sse_events(&text);
        let parsed = if events.iter().any(|e| e.parsed.is_some()) {
            Some(Value::Array(
                events.iter().filter_map(|e| e.parsed.clone()).collect(),
            ))
        } else {
            None
        };
        return ParsedTrafficBody {
            kind: TrafficContentKind::Sse,
            parsed,
            sse_events: events,
            text_clean: clean,
        };
    }

    ParsedTrafficBody {
        kind: TrafficContentKind::Raw,
        parsed: None,
        sse_events: Vec::new(),
        text_clean: text,
    }
}

fn looks_like_sse(text: &str) -> bool {
    text.lines()
        .any(|line| line.trim().starts_with("data:"))
}

fn parse_sse_events(text: &str) -> (Vec<SseEventView>, String) {
    let mut events = Vec::new();
    let mut clean_lines = Vec::new();
    let mut index = 0usize;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("data:") {
            let payload = trimmed.strip_prefix("data:").unwrap_or("").trim();
            if payload.is_empty() || payload == "[DONE]" {
                continue;
            }
            index += 1;
            let (parsed, error) = match serde_json::from_str::<Value>(payload) {
                Ok(v) => (Some(v), None),
                Err(e) => (None, Some(e.to_string())),
            };
            events.push(SseEventView {
                index,
                raw: payload.to_string(),
                parsed,
                error,
            });
            clean_lines.push(format!("data: {payload}"));
        } else if !trimmed.is_empty() {
            clean_lines.push(line.to_string());
        }
    }

    (events, clean_lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sse_events_and_strips_done() {
        let body = concat!(
            "data: {\"id\":1}\n",
            "data: [DONE]\n",
            "data: {\"id\":2}\n",
        );
        let parsed = parse_traffic_body(body.as_bytes());
        assert_eq!(parsed.kind, TrafficContentKind::Sse);
        assert_eq!(parsed.sse_events.len(), 2);
        assert_eq!(parsed.sse_events[0].index, 1);
        assert_eq!(parsed.sse_events[1].index, 2);
        assert!(!parsed.text_clean.contains("[DONE]"));
        let arr = parsed.parsed.unwrap();
        assert_eq!(arr.as_array().unwrap().len(), 2);
    }

    #[test]
    fn invalid_sse_chunk_keeps_raw_and_error() {
        let body = "data: not-json\n";
        let parsed = parse_traffic_body(body.as_bytes());
        assert_eq!(parsed.kind, TrafficContentKind::Sse);
        assert_eq!(parsed.sse_events.len(), 1);
        assert!(parsed.sse_events[0].parsed.is_none());
        assert!(parsed.sse_events[0].error.is_some());
    }

    #[test]
    fn json_body_stays_json() {
        let body = r#"{"messages":[]}"#;
        let parsed = parse_traffic_body(body.as_bytes());
        assert_eq!(parsed.kind, TrafficContentKind::Json);
        assert!(parsed.parsed.is_some());
        assert!(parsed.sse_events.is_empty());
    }
}
