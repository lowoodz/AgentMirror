use std::sync::OnceLock;

use regex::Regex;
use uuid::Uuid;

use crate::models::{CognitiveEvent, EventKind, TraceTurn};
use crate::parser::{ParsedMessage, ParsedRequest, ParsedResponse, ToolCall};

const OBS_SUMMARY_LIMIT: usize = 600;
const ACTION_ARGS_LIMIT: usize = 240;
const RESULT_SUMMARY_LIMIT: usize = 600;
const REFLECTION_SUMMARY_LIMIT: usize = 500;
const GOAL_SUMMARY_LIMIT: usize = 240;
const DECISION_SUMMARY_LIMIT: usize = 320;

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
    pub goal_emitted: bool,
}

impl ExtractContext {
    pub fn from_goal(_goal: &str, goal_already_in_run: bool) -> Self {
        Self {
            goal_emitted: goal_already_in_run,
        }
    }
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

    for msg in &req.new_messages {
        events.extend(extract_from_message(
            msg,
            &turn.audit_id,
            run_id,
            &mut seq,
            turn.timestamp,
            ctx,
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
                truncate(&resp.assistant_text, RESULT_SUMMARY_LIMIT),
                0.7,
                run_id,
                &turn.audit_id,
                &mut seq,
                turn.timestamp,
            ));
        } else if resp.tool_calls.is_empty() {
            events.push(draft(
                EventKind::Reflection,
                truncate(&resp.assistant_text, REFLECTION_SUMMARY_LIMIT),
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
        if let Some(state) = infer_state_from_tool(&call.name, &call.arguments) {
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
            truncate(&resp.assistant_text, REFLECTION_SUMMARY_LIMIT),
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
            truncate(text, GOAL_SUMMARY_LIMIT),
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
            truncate(&result.content, OBS_SUMMARY_LIMIT),
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
    let normalized = normalize_tool_name(&call.name, &call.arguments);
    let args = truncate(&call.arguments, ACTION_ARGS_LIMIT);
    if args.is_empty() {
        normalized
    } else {
        format!("{}({})", normalized, args)
    }
}

fn normalize_tool_name(name: &str, args: &str) -> String {
    let n = name.to_ascii_lowercase();
    if n == "exec" || n == "bash" || n == "shell" || n == "run_terminal_cmd" {
        let combined = format!("{name} {args}").to_ascii_lowercase();
        if combined.contains("curl")
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

fn decision_patterns() -> &'static Vec<Regex> {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            Regex::new(r"(?i)(I'll|I will|Let me|First,? I'll|我先|我将|让我先|接下来|首先)(.{5,320})")
                .unwrap(),
            Regex::new(r"(?i)(I need to|I should|我需要|我应该|打算|计划)(.{5,320})").unwrap(),
            Regex::new(
                r"(先|首先|接下来).{4,160}(查询|搜索|了解|收集|分析|检查|尝试|处理|实现|修复|编写|运行|调用)",
            )
            .unwrap(),
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
                return Some(truncate(&phrase, DECISION_SUMMARY_LIMIT));
            }
        }
    }
    None
}

fn looks_like_result(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if lower.contains("completed")
        || lower.contains("done")
        || lower.contains("fixed")
        || lower.contains("success")
        || lower.contains("resolved")
        || text.contains("完成")
        || text.contains("已修复")
        || text.contains("搞定")
        || text.contains("已完成")
        || text.contains("解决了")
    {
        return true;
    }
    let conclusion_markers = [
        "结论",
        "总结",
        "综上",
        "总体而言",
        "综合来看",
        "最后",
        "总之",
        "conclusion",
        "in summary",
        "to summarize",
        "in conclusion",
        "overall",
    ];
    if conclusion_markers
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
    false
}

fn infer_state_from_tool(name: &str, args: &str) -> Option<String> {
    let normalized = normalize_tool_name(name, args);
    let n = normalized.to_ascii_lowercase();
    if n.contains("search") || n.contains("web") || n.contains("browse") {
        Some("Information gathering".to_string())
    } else if n.contains("read") || n.contains("grep") || n.contains("list") {
        Some("Information gathering".to_string())
    } else if n.contains("edit") || n.contains("write") || n.contains("patch") {
        Some("Implementation".to_string())
    } else if n.contains("test") || n == "exec" {
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn detects_task_conclusion_in_chinese() {
        let text = "综合以上信息，该方案在性能上具备优势，但集成成本偏高。结论：可以试点，暂不建议全面推广。";
        assert!(looks_like_result(text));
        let interim = "接下来我会从配置和依赖两方面继续检查，综合来看还需要更多数据。";
        assert!(!looks_like_result(interim));
    }

    #[test]
    fn normalizes_exec_with_http_args_to_web_search() {
        let call = ToolCall {
            name: "exec".into(),
            arguments: r#"{"command":"curl https://example.com/search?q=topic"}"#.into(),
        };
        assert_eq!(
            format_action(&call),
            "WebSearch({\"command\":\"curl https://example.com/search?q=topic\"})"
        );
    }

    #[test]
    fn emits_single_goal_then_subgoal() {
        let mut ctx = ExtractContext {
            goal_emitted: false,
        };
        let msg1 = ParsedMessage {
            role: "user".into(),
            text: "整理项目依赖并输出报告".into(),
            tool_calls: vec![],
            tool_results: vec![],
        };
        let msg2 = ParsedMessage {
            role: "user".into(),
            text: "再检查一下测试覆盖率".into(),
            tool_calls: vec![],
            tool_results: vec![],
        };
        let e1 = extract_from_message(&msg1, "a", "r", &mut 0, Utc::now(), &mut ctx);
        let e2 = extract_from_message(&msg2, "a", "r", &mut 1, Utc::now(), &mut ctx);
        assert_eq!(e1[0].kind, EventKind::Goal);
        assert_eq!(e2[0].kind, EventKind::SubGoal);
    }

    #[test]
    fn detects_decision_without_industry_keywords() {
        let text = "我先读取配置文件，然后运行测试验证修改。";
        assert!(extract_decision(text).is_some());
    }
}
