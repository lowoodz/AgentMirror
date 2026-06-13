use std::sync::OnceLock;

use regex::Regex;
use uuid::Uuid;

use crate::models::{CognitiveEvent, EventKind, TraceTurn};
use crate::parser::{ParsedMessage, ParsedRequest, ParsedResponse, ToolCall};

pub struct ExtractedTurn {
    pub events: Vec<CognitiveEventDraft>,
}

pub struct CognitiveEventDraft {
    pub kind: EventKind,
    pub summary: String,
    pub confidence: f32,
    pub metadata: serde_json::Value,
}

pub fn extract_from_turn(
    turn: &TraceTurn,
    req: &ParsedRequest,
    resp: &ParsedResponse,
    run_id: &str,
    start_seq: u32,
) -> ExtractedTurn {
    let mut events = Vec::new();
    let mut seq = start_seq;

    for msg in &req.new_messages {
        events.extend(extract_from_message(
            msg,
            &turn.audit_id,
            run_id,
            &mut seq,
            turn.timestamp,
        ));
    }

    if !resp.assistant_text.trim().is_empty() {
        if let Some(decision) = extract_decision(&resp.assistant_text) {
            events.push(draft(
                EventKind::Decision,
                decision,
                0.75,
                run_id,
                &turn.audit_id,
                &mut seq,
                turn.timestamp,
            ));
        }
        if looks_like_result(&resp.assistant_text) {
            events.push(draft(
                EventKind::Result,
                truncate(&resp.assistant_text, 200),
                0.7,
                run_id,
                &turn.audit_id,
                &mut seq,
                turn.timestamp,
            ));
        } else if resp.tool_calls.is_empty() {
            events.push(draft(
                EventKind::Reflection,
                truncate(&resp.assistant_text, 200),
                0.6,
                run_id,
                &turn.audit_id,
                &mut seq,
                turn.timestamp,
            ));
        }
    }

    for call in &resp.tool_calls {
        events.push(draft(
            EventKind::Action,
            format_action(call),
            0.95,
            run_id,
            &turn.audit_id,
            &mut seq,
            turn.timestamp,
        ));
        if let Some(state) = infer_state_from_tool(&call.name) {
            events.push(draft(
                EventKind::StateTransition,
                state,
                0.8,
                run_id,
                &turn.audit_id,
                &mut seq,
                turn.timestamp,
            ));
        }
    }

    if events.is_empty() && !resp.assistant_text.is_empty() {
        events.push(draft(
            EventKind::Reflection,
            truncate(&resp.assistant_text, 200),
            0.5,
            run_id,
            &turn.audit_id,
            &mut seq,
            turn.timestamp,
        ));
    }

    ExtractedTurn { events }
}

fn extract_from_message(
    msg: &ParsedMessage,
    audit_id: &str,
    run_id: &str,
    seq: &mut u32,
    ts: chrono::DateTime<chrono::Utc>,
) -> Vec<CognitiveEventDraft> {
    let mut out = Vec::new();
    if msg.role == "user" && !msg.text.trim().is_empty() {
        out.push(draft(
            EventKind::Goal,
            truncate(msg.text.trim(), 120),
            0.85,
            run_id,
            audit_id,
            seq,
            ts,
        ));
    }
    for call in &msg.tool_calls {
        out.push(draft(
            EventKind::Action,
            format_action(call),
            0.95,
            run_id,
            audit_id,
            seq,
            ts,
        ));
    }
    for result in &msg.tool_results {
        out.push(draft(
            EventKind::Observation,
            truncate(&result.content, 200),
            0.9,
            run_id,
            audit_id,
            seq,
            ts,
        ));
    }
    if msg.role == "assistant" {
        if let Some(decision) = extract_decision(&msg.text) {
            out.push(draft(
                EventKind::Decision,
                decision,
                0.75,
                run_id,
                audit_id,
                seq,
                ts,
            ));
        }
    }
    out
}

fn draft(
    kind: EventKind,
    summary: String,
    confidence: f32,
    _run_id: &str,
    _audit_id: &str,
    _seq: &mut u32,
    _ts: chrono::DateTime<chrono::Utc>,
) -> CognitiveEventDraft {
    CognitiveEventDraft {
        kind,
        summary,
        confidence,
        metadata: serde_json::Value::Null,
    }
}

pub fn drafts_to_events(
    drafts: Vec<CognitiveEventDraft>,
    run_id: &str,
    audit_id: &str,
    start_seq: u32,
    ts: chrono::DateTime<chrono::Utc>,
) -> Vec<CognitiveEvent> {
    drafts
        .into_iter()
        .enumerate()
        .map(|(i, d)| CognitiveEvent {
            id: Uuid::new_v4().to_string(),
            run_id: run_id.to_string(),
            seq: start_seq + i as u32,
            kind: d.kind,
            timestamp: ts,
            summary: d.summary,
            audit_id: audit_id.to_string(),
            confidence: d.confidence,
            metadata: d.metadata,
        })
        .collect()
}

fn format_action(call: &ToolCall) -> String {
    let args = truncate(&call.arguments, 80);
    if args.is_empty() {
        call.name.clone()
    } else {
        format!("{}({})", call.name, args)
    }
}

fn decision_patterns() -> &'static Vec<Regex> {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            Regex::new(r"(?i)(I'll|I will|Let me|First,? I'll|我先|我将|让我先)(.{5,120})").unwrap(),
            Regex::new(r"(?i)(I need to|I should|我需要|我应该)(.{5,120})").unwrap(),
        ]
    })
}

fn extract_decision(text: &str) -> Option<String> {
    for re in decision_patterns().iter() {
        if let Some(caps) = re.captures(text) {
            let phrase = caps
                .get(0)
                .map(|m| m.as_str().trim())
                .unwrap_or("")
                .to_string();
            if phrase.len() > 8 {
                return Some(truncate(&phrase, 120));
            }
        }
    }
    None
}

fn looks_like_result(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("completed")
        || lower.contains("done")
        || lower.contains("fixed")
        || lower.contains("success")
        || text.contains("完成")
        || text.contains("已修复")
        || text.contains("搞定")
}

fn infer_state_from_tool(name: &str) -> Option<String> {
    let n = name.to_ascii_lowercase();
    if n.contains("read") || n.contains("search") || n.contains("grep") || n.contains("list") {
        Some("Information gathering".to_string())
    } else if n.contains("edit") || n.contains("write") || n.contains("patch") {
        Some("Implementation".to_string())
    } else if n.contains("test") || n.contains("bash") || n.contains("run") {
        Some("Verification".to_string())
    } else {
        None
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}
