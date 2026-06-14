use chrono::{DateTime, Utc};
use uuid::Uuid;
use xxhash_rust::xxh64::xxh64;

use crate::models::{AgentRecord, RunRecord, RunStatus};
use crate::parser::ParsedRequest;

const AGENT_NS: Uuid = Uuid::NAMESPACE_OID;

pub struct AgentContext {
    pub agent_id: String,
    pub agent_record: AgentRecord,
}

pub fn resolve_agent(
    turn: &crate::models::TraceTurn,
    req: &ParsedRequest,
    existing_agent: Option<&AgentRecord>,
) -> AgentContext {
    let agent_id = if let Some(header) = turn.agent_header.as_deref().filter(|s| !s.is_empty()) {
        format!("hdr-{}", short_hash(header.as_bytes()))
    } else {
        let fp = agent_fingerprint(&req.system_excerpt, &req.tools);
        format!("fp-{}", short_hash(fp.as_bytes()))
    };

    let agent_type = infer_agent_type(&req.system_excerpt, &req.tools, infer_goal_from_request(req));
    let display_name = if let Some(existing) = existing_agent {
        existing.display_name.clone()
    } else {
        infer_display_name(&agent_type, &req.tools)
    };

    let now = turn.timestamp;
    let record = AgentRecord {
        agent_id: agent_id.clone(),
        display_name,
        agent_type: agent_type.clone(),
        system_hash: short_hash(req.system_excerpt.as_bytes()),
        tools_json: serde_json::to_string(&req.tools).unwrap_or_else(|_| "[]".to_string()),
        first_seen: existing_agent.map(|a| a.first_seen).unwrap_or(now),
        last_seen: now,
    };

    AgentContext {
        agent_id,
        agent_record: record,
    }
}

fn agent_fingerprint(system: &str, tools: &[String]) -> String {
    let mut parts = vec![format!("sys:{system}")];
    if !tools.is_empty() {
        parts.push(format!("tools:{}", tools.join(",")));
    }
    parts.join("\n")
}

fn short_hash(data: &[u8]) -> String {
    format!("{:016x}", xxh64(data, 0))
}

fn infer_agent_type(system: &str, tools: &[String], goal: String) -> String {
    let lower = system.to_ascii_lowercase();
    let tool_str = tools.join(" ").to_ascii_lowercase();
    let goal_lower = goal.to_ascii_lowercase();
    if crate::extract::is_research_goal(&goal) {
        return "research".to_string();
    }
    if lower.contains("claude code") || tool_str.contains("edit") && tool_str.contains("bash") {
        "coding".to_string()
    } else if tool_str.contains("browser") || tool_str.contains("search") || tool_str.contains("web") {
        "research".to_string()
    } else if tools.iter().any(|t| {
        let n = t.to_ascii_lowercase();
        n == "exec" || n == "bash" || n == "shell"
    }) && (goal.contains("调研") || goal.contains("研究") || goal_lower.contains("research")) {
        "research".to_string()
    } else if tools.is_empty() {
        "chat".to_string()
    } else {
        "agent".to_string()
    }
}

fn infer_display_name(agent_type: &str, tools: &[String]) -> String {
    match agent_type {
        "coding" => "Coding Agent".to_string(),
        "research" => "Research Agent".to_string(),
        "chat" => "Chat Agent".to_string(),
        _ => {
            if tools.is_empty() {
                "Agent".to_string()
            } else {
                format!("Agent ({})", tools.first().unwrap_or(&"tools".to_string()))
            }
        }
    }
}

/// Idle gap after which the next turn starts a new Run (see detailed plan §十二).
const RUN_IDLE_MINUTES: i64 = 30;

const NEW_TASK_MARKERS: &[&str] = &[
    "new task:",
    "/clear",
    "start over",
    "forget the previous",
    "新任务",
    "另一个任务",
    "重新开始",
];

/// Decide whether to open a new Run vs continue the active one for this agent+session.
pub fn should_start_new_run(
    req: &ParsedRequest,
    active_run: Option<&RunRecord>,
    turn_time: DateTime<Utc>,
) -> bool {
    let Some(run) = active_run else {
        return true;
    };

    if run.status != RunStatus::Running {
        return true;
    }

    if let Some(ended) = run.ended_at {
        if turn_time.signed_duration_since(ended).num_minutes() > RUN_IDLE_MINUTES {
            return true;
        }
    }

    if let Some(user_text) = latest_user_message(req) {
        if looks_like_new_task(user_text, &run.goal) {
            return true;
        }
    }

    false
}

fn latest_user_message(req: &ParsedRequest) -> Option<&str> {
    req.new_messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.text.as_str())
        .filter(|t| !t.trim().is_empty())
}

fn looks_like_new_task(user_text: &str, goal: &str) -> bool {
    let lower = user_text.to_ascii_lowercase();
    if NEW_TASK_MARKERS.iter().any(|m| lower.contains(m)) {
        return true;
    }
    // Strong topic shift: long message with almost no keyword overlap with current goal.
    if user_text.chars().count() > 60 && keyword_jaccard(goal, user_text) < 0.12 {
        return true;
    }
    false
}

fn keyword_jaccard(a: &str, b: &str) -> f32 {
    let set_a = keyword_set(a);
    let set_b = keyword_set(b);
    if set_a.is_empty() || set_b.is_empty() {
        return 0.0;
    }
    let inter = set_a.intersection(&set_b).count() as f32;
    let union = set_a.union(&set_b).count() as f32;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

fn keyword_set(text: &str) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    let lower = text.to_ascii_lowercase();
    for word in lower.split(|c: char| !c.is_alphanumeric()) {
        if word.len() >= 3 {
            set.insert(word.to_string());
        }
    }
    for token in chinese_tokens(text) {
        set.insert(token);
    }
    set
}

fn chinese_tokens(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().filter(|c| is_cjk(*c)).collect();
    if chars.len() < 2 {
        return Vec::new();
    }
    chars
        .windows(2)
        .map(|w| w.iter().collect::<String>())
        .collect()
}

fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{4E00}'..='\u{9FFF}'
            | '\u{3400}'..='\u{4DBF}'
            | '\u{F900}'..='\u{FAFF}'
    )
}

pub fn new_run_id(session_id: &str, agent_id: &str) -> String {
    let seed = format!("{session_id}:{agent_id}:{}", Utc::now().timestamp_nanos_opt().unwrap_or(0));
    Uuid::new_v5(&AGENT_NS, seed.as_bytes()).to_string()
}

pub fn infer_goal_from_request(req: &ParsedRequest) -> String {
    for msg in &req.new_messages {
        if msg.role == "user" {
            let text = msg.text.trim();
            if !text.is_empty() {
                return truncate_goal(text);
            }
        }
    }
    if !req.system_excerpt.is_empty() {
        return truncate_goal(&req.system_excerpt);
    }
    "Unknown task".to_string()
}

fn truncate_goal(s: &str) -> String {
    let s = s.lines().next().unwrap_or(s).trim();
    if s.chars().count() > 120 {
        format!("{}…", s.chars().take(120).collect::<String>())
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use crate::models::TraceTurn;
    use crate::parser::parse_request;
    use crate::models::RunRecord;

    #[test]
    fn stable_agent_id_for_same_fingerprint() {
        let body = br#"{"messages":[{"role":"system","content":"You are Claude Code"},{"role":"user","content":"fix bug"}],"tools":[{"type":"function","function":{"name":"Read"}}]}"#;
        let req = parse_request(body);
        let turn = TraceTurn {
            audit_id: "a1".into(),
            session_id: "s1".into(),
            agent_header: None,
            timestamp: Utc::now(),
            request_body: body.to_vec(),
            response_body: vec![],
        };
        let ctx1 = resolve_agent(&turn, &req, None);
        let ctx2 = resolve_agent(&turn, &req, None);
        assert_eq!(ctx1.agent_id, ctx2.agent_id);
    }

    #[test]
    fn continues_run_for_tool_only_turn() {
        let req = parse_request(b"{\"messages\":[{\"role\":\"user\",\"content\":\"fix bug\"},{\"role\":\"assistant\",\"content\":\"ok\"},{\"role\":\"tool\",\"content\":\"file contents\"}]}");
        let run = RunRecord {
            run_id: "r1".into(),
            agent_id: "a1".into(),
            session_id: "s1".into(),
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
            status: RunStatus::Running,
            goal: "fix bug".into(),
            turn_count: 1,
            messages_seen: 3,
            graph_path: None,
        };
        assert!(!should_start_new_run(&req, Some(&run), Utc::now()));
    }

    #[test]
    fn infers_research_agent_for_exec_tools_and_goal() {
        let body = r#"{"messages":[{"role":"user","content":"帮我调研珠海金智维，看看是否值得投资"}],"tools":[{"type":"function","function":{"name":"exec"}}]}"#.as_bytes();
        let req = parse_request(body);
        let turn = TraceTurn {
            audit_id: "a1".into(),
            session_id: "s1".into(),
            agent_header: None,
            timestamp: Utc::now(),
            request_body: body.to_vec(),
            response_body: vec![],
        };
        let ctx = resolve_agent(&turn, &req, None);
        assert_eq!(ctx.agent_record.agent_type, "research");
    }

    #[test]
    fn chinese_topic_shift_stays_in_run_when_related() {
        let req = parse_request(
            r#"{"messages":[{"role":"user","content":"帮我调研珠海金智维融资情况"}]}"#.as_bytes(),
        );
        let run = RunRecord {
            run_id: "r1".into(),
            agent_id: "a1".into(),
            session_id: "s1".into(),
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
            status: RunStatus::Running,
            goal: "帮我调研珠海金智维，看看是否值得投资".into(),
            turn_count: 2,
            messages_seen: 1,
            graph_path: None,
        };
        assert!(!should_start_new_run(&req, Some(&run), Utc::now()));
    }

    #[test]
    fn new_run_after_idle_timeout() {
        let req = parse_request(b"{\"messages\":[{\"role\":\"user\",\"content\":\"fix bug\"}]}");
        let run = RunRecord {
            run_id: "r1".into(),
            agent_id: "a1".into(),
            session_id: "s1".into(),
            started_at: Utc::now() - chrono::Duration::minutes(45),
            ended_at: Some(Utc::now() - chrono::Duration::minutes(45)),
            status: RunStatus::Running,
            goal: "fix bug".into(),
            turn_count: 1,
            messages_seen: 1,
            graph_path: None,
        };
        assert!(should_start_new_run(&req, Some(&run), Utc::now()));
    }

    #[test]
    fn new_run_on_explicit_marker() {
        let req = parse_request(b"{\"messages\":[{\"role\":\"user\",\"content\":\"new task: write docs\"}]}");
        let run = RunRecord {
            run_id: "r1".into(),
            agent_id: "a1".into(),
            session_id: "s1".into(),
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
            status: RunStatus::Running,
            goal: "fix bug".into(),
            turn_count: 2,
            messages_seen: 2,
            graph_path: None,
        };
        assert!(should_start_new_run(&req, Some(&run), Utc::now()));
    }
}
