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

pub struct ExtractContext {
    pub research_task: bool,
    pub goal_emitted: bool,
}

impl ExtractContext {
    pub fn from_goal(goal: &str, goal_already_in_run: bool) -> Self {
        Self {
            research_task: is_research_goal(goal),
            goal_emitted: goal_already_in_run,
        }
    }
}

pub fn is_research_goal(text: &str) -> bool {
    let markers = [
        "调研", "研究", "分析", "投资", "报告", "评估", "是否值得", "竞品", "市场",
        "research", "invest", "due diligence", "market analysis",
    ];
    let lower = text.to_ascii_lowercase();
    markers
        .iter()
        .any(|m| text.contains(m) || lower.contains(&m.to_ascii_lowercase()))
}

pub fn extract_from_turn(
    turn: &TraceTurn,
    req: &ParsedRequest,
    resp: &ParsedResponse,
    run_id: &str,
    start_seq: u32,
    ctx: &mut ExtractContext,
) -> ExtractedTurn {
    let mut events = Vec::new();
    let mut seq = start_seq;
    let obs_limit = if ctx.research_task { 400 } else { 200 };
    let action_limit = if ctx.research_task { 160 } else { 80 };

    for msg in &req.new_messages {
        events.extend(extract_from_message(
            msg,
            &turn.audit_id,
            run_id,
            &mut seq,
            turn.timestamp,
            ctx,
            obs_limit,
            action_limit,
        ));
    }

    if !resp.assistant_text.trim().is_empty() {
        if let Some(decision) = extract_decision(&resp.assistant_text, ctx.research_task) {
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
        if looks_like_result(&resp.assistant_text, ctx.research_task) {
            events.push(draft(
                EventKind::Result,
                truncate(&resp.assistant_text, if ctx.research_task { 400 } else { 200 }),
                0.7,
                run_id,
                &turn.audit_id,
                &mut seq,
                turn.timestamp,
            ));
        } else if resp.tool_calls.is_empty() {
            events.push(draft(
                EventKind::Reflection,
                truncate(&resp.assistant_text, if ctx.research_task { 300 } else { 200 }),
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
            format_action(call, ctx.research_task, action_limit),
            0.95,
            run_id,
            &turn.audit_id,
            &mut seq,
            turn.timestamp,
        ));
        if let Some(state) = infer_state_from_tool(&call.name, &call.arguments, ctx.research_task) {
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
    ctx: &mut ExtractContext,
    obs_limit: usize,
    action_limit: usize,
) -> Vec<CognitiveEventDraft> {
    let mut out = Vec::new();
    if msg.role == "user" && !msg.text.trim().is_empty() {
        let text = msg.text.trim();
        let kind = if ctx.goal_emitted {
            EventKind::SubGoal
        } else {
            ctx.goal_emitted = true;
            EventKind::Goal
        };
        out.push(draft(
            kind,
            truncate(text, 120),
            if kind == EventKind::Goal { 0.85 } else { 0.75 },
            run_id,
            audit_id,
            seq,
            ts,
        ));
    }
    for call in &msg.tool_calls {
        out.push(draft(
            EventKind::Action,
            format_action(call, ctx.research_task, action_limit),
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
            truncate(&result.content, obs_limit),
            0.9,
            run_id,
            audit_id,
            seq,
            ts,
        ));
    }
    if msg.role == "assistant" {
        if let Some(decision) = extract_decision(&msg.text, ctx.research_task) {
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

fn format_action(call: &ToolCall, research_task: bool, max_args: usize) -> String {
    let normalized = normalize_tool_name(&call.name, &call.arguments, research_task);
    let args = truncate(&call.arguments, max_args);
    if args.is_empty() {
        normalized
    } else {
        format!("{}({})", normalized, args)
    }
}

fn normalize_tool_name(name: &str, args: &str, research_task: bool) -> String {
    let n = name.to_ascii_lowercase();
    if n == "exec" || n == "bash" || n == "shell" || n == "run_terminal_cmd" {
        let combined = format!("{name} {args}").to_ascii_lowercase();
        if research_task
            || combined.contains("curl")
            || combined.contains("wget")
            || combined.contains("search")
            || combined.contains("google")
            || combined.contains("bing")
            || combined.contains("http")
        {
            return "WebSearch".to_string();
        }
        return "Exec".to_string();
    }
    if n.contains("search") || n.contains("web") || n.contains("browse") {
        return "WebSearch".to_string();
    }
    name.to_string()
}

fn decision_patterns(research: bool) -> &'static Vec<Regex> {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    static RESEARCH_PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    if research {
        RESEARCH_PATTERNS.get_or_init(|| {
            vec![
                Regex::new(r"(?i)(I'll|I will|Let me|First,? I'll|我先|我将|让我先|接下来|首先)(.{5,160})").unwrap(),
                Regex::new(r"(?i)(I need to|I should|我需要|我应该|打算|计划)(.{5,160})").unwrap(),
                Regex::new(r"(先|首先|接下来).{4,80}(调研|查询|搜索|了解|收集|分析)").unwrap(),
            ]
        })
    } else {
        PATTERNS.get_or_init(|| {
            vec![
                Regex::new(r"(?i)(I'll|I will|Let me|First,? I'll|我先|我将|让我先)(.{5,120})").unwrap(),
                Regex::new(r"(?i)(I need to|I should|我需要|我应该)(.{5,120})").unwrap(),
            ]
        })
    }
}

fn extract_decision(text: &str, research: bool) -> Option<String> {
    for re in decision_patterns(research).iter() {
        if let Some(caps) = re.captures(text) {
            let phrase = caps
                .get(0)
                .map(|m| m.as_str().trim())
                .unwrap_or("")
                .to_string();
            if phrase.len() > 8 {
                return Some(truncate(&phrase, 160));
            }
        }
    }
    None
}

fn looks_like_result(text: &str, research: bool) -> bool {
    let lower = text.to_ascii_lowercase();
    if lower.contains("completed")
        || lower.contains("done")
        || lower.contains("fixed")
        || lower.contains("success")
        || text.contains("完成")
        || text.contains("已修复")
        || text.contains("搞定")
    {
        return true;
    }
        if research {
        let strong_markers = ["结论", "投资建议", "是否值得", "不推荐", "推荐", "conclusion", "investment thesis"];
        if strong_markers
            .iter()
            .any(|m| text.contains(m) || lower.contains(&m.to_ascii_lowercase()))
        {
            let min_len = if text.contains("结论") || lower.contains("conclusion") {
                40
            } else {
                80
            };
            return text.chars().count() >= min_len;
        }
        let research_markers = [
            "总结", "综合来看", "总体而言",
        ];
        if research_markers
            .iter()
            .any(|m| text.contains(m) || lower.contains(&m.to_ascii_lowercase()))
        {
            return text.chars().count() >= 120;
        }
    }
    false
}

fn infer_state_from_tool(name: &str, args: &str, research_task: bool) -> Option<String> {
    let normalized = normalize_tool_name(name, args, research_task);
    let n = normalized.to_ascii_lowercase();
    if n.contains("search") || n.contains("web") || n.contains("browse") {
        Some("Information gathering".to_string())
    } else if n.contains("read") || n.contains("grep") || n.contains("list") {
        Some(if research_task {
            "Information gathering".to_string()
        } else {
            "Information gathering".to_string()
        })
    } else if n.contains("edit") || n.contains("write") || n.contains("patch") {
        Some("Implementation".to_string())
    } else if n.contains("test") || n.contains("bash") || n.contains("exec") {
        Some(if research_task {
            "Information gathering".to_string()
        } else {
            "Verification".to_string()
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn detects_research_result_in_chinese() {
        let text = "综合以上调研，珠海金智维在 RPA 领域具备一定优势，但估值偏高。结论：谨慎关注，暂不建议重仓投资。";
        assert!(looks_like_result(text, true));
        let interim = "接下来我会从融资、竞争格局两方面继续搜索，综合来看需要更多一手资料。";
        assert!(!looks_like_result(interim, true));
    }

    #[test]
    fn normalizes_exec_to_web_search_for_research() {
        let call = ToolCall {
            name: "exec".into(),
            arguments: r#"{"command":"curl https://example.com/search?q=金智维"}"#.into(),
        };
        assert_eq!(
            format_action(&call, true, 160),
            "WebSearch({\"command\":\"curl https://example.com/search?q=金智维\"})"
        );
    }

    #[test]
    fn emits_single_goal_then_subgoal() {
        let mut ctx = ExtractContext {
            research_task: true,
            goal_emitted: false,
        };
        let msg1 = ParsedMessage {
            role: "user".into(),
            text: "调研珠海金智维".into(),
            tool_calls: vec![],
            tool_results: vec![],
        };
        let msg2 = ParsedMessage {
            role: "user".into(),
            text: "再查一下融资情况".into(),
            tool_calls: vec![],
            tool_results: vec![],
        };
        let e1 = extract_from_message(&msg1, "a", "r", &mut 0, Utc::now(), &mut ctx, 400, 160);
        let e2 = extract_from_message(&msg2, "a", "r", &mut 1, Utc::now(), &mut ctx, 400, 160);
        assert_eq!(e1[0].kind, EventKind::Goal);
        assert_eq!(e2[0].kind, EventKind::SubGoal);
    }
}
