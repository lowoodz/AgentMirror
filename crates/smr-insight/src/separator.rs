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

    let platform = detect_platform(
        &req.system_excerpt,
        &req.tools,
        turn.agent_header.as_deref(),
        req.model.as_deref(),
    );
    let agent_type = infer_agent_type(
        &req.system_excerpt,
        &req.tools,
        infer_goal_from_request(req),
        &platform,
    );
    let display_name = platform.display_name.to_string();

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
    if !tools.is_empty() {
        let mut sorted: Vec<_> = tools.iter().map(|t| t.to_ascii_lowercase()).collect();
        sorted.sort();
        sorted.dedup();
        return format!("tools:{}", sorted.join(","));
    }
    format!("sys:{system}")
}

fn short_hash(data: &[u8]) -> String {
    format!("{:016x}", xxh64(data, 0))
}

fn infer_agent_type(system: &str, tools: &[String], _goal: String, platform: &PlatformInfo) -> String {
    let lower = system.to_ascii_lowercase();
    let tool_str = tools.join(" ").to_ascii_lowercase();
    if platform.platform_id == "claude_code"
        || platform.platform_id == "codex"
        || platform.platform_id == "aider"
        || platform.platform_id == "cline"
        || lower.contains("claude code")
        || tool_str.contains("edit") && tool_str.contains("bash")
    {
        "coding".to_string()
    } else if tool_str.contains("browser") || tool_str.contains("search") || tool_str.contains("web") {
        "explore".to_string()
    } else if platform.platform_id == "openclaw" || platform.platform_id == "hermes" {
        "explore".to_string()
    } else if tools.iter().any(|t| {
        let n = t.to_ascii_lowercase();
        n == "exec" || n == "bash" || n == "shell"
    }) && !tool_str.contains("edit") && !tool_str.contains("write") {
        "explore".to_string()
    } else if tools.is_empty() {
        "chat".to_string()
    } else {
        "agent".to_string()
    }
}

struct PlatformInfo {
    platform_id: &'static str,
    display_name: &'static str,
}

fn detect_platform(
    system: &str,
    tools: &[String],
    agent_header: Option<&str>,
    model: Option<&str>,
) -> PlatformInfo {
    let lower = system.to_ascii_lowercase();
    let tool_str = tools.join(" ").to_ascii_lowercase();
    let header = agent_header.unwrap_or("").to_ascii_lowercase();
    let model_lower = model.unwrap_or("").to_ascii_lowercase();
    let combined = format!("{lower} {header} {model_lower}");

    const PLATFORMS: &[(&str, &str, &[&str])] = &[
        ("openclaw", "OpenClaw", &["openclaw", "open claw"]),
        ("hermes", "Hermes", &["hermes"]),
        ("claude_code", "Claude Code", &["claude code", "claude-code"]),
        ("codex", "Codex", &["codex", "openai codex"]),
        ("cursor", "Cursor", &["cursor agent", "cursor tab", "cursor"]),
        ("windsurf", "Windsurf", &["windsurf", "cascade"]),
        ("copilot", "GitHub Copilot", &["github copilot", "copilot agent"]),
        ("aider", "Aider", &["aider"]),
        ("cline", "Cline", &["cline", "claude dev"]),
        ("gemini", "Gemini", &["gemini cli", "google gemini"]),
        ("chatgpt", "ChatGPT", &["chatgpt agent", "chatgpt"]),
    ];

    for (id, name, markers) in PLATFORMS {
        if markers.iter().any(|m| combined.contains(m)) {
            return PlatformInfo {
                platform_id: id,
                display_name: name,
            };
        }
    }

    if tools.iter().any(|t| t.eq_ignore_ascii_case("exec"))
        && !tool_str.contains("read")
        && !tool_str.contains("edit")
        && !tool_str.contains("apply_patch")
    {
        return PlatformInfo {
            platform_id: "openclaw",
            display_name: "OpenClaw",
        };
    }

    if tool_str.contains("read")
        && (tool_str.contains("edit") || tool_str.contains("write") || tool_str.contains("bash"))
    {
        return PlatformInfo {
            platform_id: "claude_code",
            display_name: "Claude Code",
        };
    }

    if tools.is_empty() {
        PlatformInfo {
            platform_id: "chat",
            display_name: "Chat",
        }
    } else {
        PlatformInfo {
            platform_id: "agent",
            display_name: "Agent",
        }
    }
}

#[cfg(test)]
mod platform_tests {
    use super::*;

    #[test]
    fn detects_openclaw_from_system_and_exec_tools() {
        let p = detect_platform(
            "You are OpenClaw, a personal assistant.",
            &["exec".to_string()],
            None,
            None,
        );
        assert_eq!(p.display_name, "OpenClaw");
    }

    #[test]
    fn detects_claude_code_from_tools() {
        let p = detect_platform(
            "You are a software engineer.",
            &["Read".to_string(), "Edit".to_string(), "Bash".to_string()],
            None,
            None,
        );
        assert_eq!(p.display_name, "Claude Code");
    }

    #[test]
    fn detects_codex_from_model() {
        let p = detect_platform("", &[], None, Some("codex-mini"));
        assert_eq!(p.display_name, "Codex");
    }

    #[test]
    fn exec_only_defaults_to_openclaw() {
        let p = detect_platform("assistant", &["exec".to_string()], None, None);
        assert_eq!(p.display_name, "OpenClaw");
    }
}

/// Idle gap after which the next turn starts a new Run (see detailed plan §十二).
pub const RUN_IDLE_MINUTES: i64 = 30;

/// Whether a prior run is still within the continuation window for this turn.
pub fn run_continue_window(turn_time: DateTime<Utc>, run: &RunRecord) -> bool {
    if run.status == RunStatus::Failed {
        return false;
    }
    let anchor = run.ended_at.unwrap_or(run.started_at);
    turn_time.signed_duration_since(anchor).num_minutes() <= RUN_IDLE_MINUTES
}

/// True when two goals likely describe the same task (dedup fragmented runs).
pub fn goals_related(a: &str, b: &str) -> bool {
    let a = a.trim();
    let b = b.trim();
    if a.is_empty() || b.is_empty() || a == "Unknown task" || b == "Unknown task" {
        return false;
    }
    if a == b || a.contains(b) || b.contains(a) {
        return true;
    }
    keyword_jaccard(a, b) >= 0.25
}

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

    if run.status == RunStatus::Failed {
        return true;
    }

    if run.status != RunStatus::Running {
        if let Some(ended) = run.ended_at {
            if turn_time.signed_duration_since(ended).num_minutes() <= RUN_IDLE_MINUTES {
                if let Some(user_text) = latest_real_user_message(req) {
                    if looks_like_new_task(user_text, &run.goal) {
                        return true;
                    }
                }
                return false;
            }
        }
        return true;
    }

    if let Some(ended) = run.ended_at {
        if turn_time.signed_duration_since(ended).num_minutes() > RUN_IDLE_MINUTES {
            return true;
        }
    }

    if let Some(user_text) = latest_real_user_message(req) {
        if looks_like_new_task(user_text, &run.goal) {
            return true;
        }
    }

    false
}

/// OpenClaw injects a bootstrap user turn whose timestamp changes every request.
pub fn is_bootstrap_user_message(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("a new session was started via /new or /reset")
        || lower.contains("session startup sequence")
        || lower.contains("bootstrap.md")
        || text.contains("[Bootstrap pending]")
}

pub fn is_bootstrap_goal(text: &str) -> bool {
    is_bootstrap_user_message(text) || text.trim().eq_ignore_ascii_case("unknown task")
}

/// Placeholder goals that should be replaced when a real user task appears.
pub fn is_weak_goal(text: &str) -> bool {
    let t = text.trim();
    is_bootstrap_goal(t)
        || t.starts_with("You are ")
        || t.eq_ignore_ascii_case("openclaw")
        || t.chars().count() < 8
}

fn latest_real_user_message(req: &ParsedRequest) -> Option<&str> {
    req.new_messages
        .iter()
        .rev()
        .find(|m| m.role == "user" && !is_bootstrap_user_message(&m.text))
        .map(|m| m.text.as_str())
        .filter(|t| !t.trim().is_empty())
}

fn looks_like_new_task(user_text: &str, goal: &str) -> bool {
    if is_bootstrap_user_message(user_text) {
        return false;
    }
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

/// Explicit task switches should open a new run, not merge into the prior goal.
pub fn skip_run_deduplication(req: &ParsedRequest, prior_goal: Option<&str>) -> bool {
    if let Some(user_text) = latest_real_user_message(req) {
        return looks_like_new_task(user_text, prior_goal.unwrap_or(""));
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
    for msg in req.new_messages.iter().rev() {
        if msg.role != "user" {
            continue;
        }
        let text = msg.text.trim();
        if text.is_empty() || is_bootstrap_user_message(text) {
            continue;
        }
        return truncate_goal(text);
    }
    if !req.system_excerpt.is_empty() && !is_bootstrap_user_message(&req.system_excerpt) {
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
    fn bootstrap_user_message_is_not_new_task() {
        let goal = "A new session was started via /new or /reset. Execute your Session Startup";
        let later = "A new session was started via /new or /reset. Execute your Session Startup sequence now. Current time: Sunday - 5:02 PM";
        assert!(!looks_like_new_task(later, goal));
        assert!(is_bootstrap_user_message(later));
    }

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
    fn stable_agent_id_when_system_grows() {
        let body1 = br#"{"messages":[{"role":"system","content":"You are OpenClaw v1"},{"role":"user","content":"task"}],"tools":[{"type":"function","function":{"name":"exec"}}]}"#;
        let body2 = br#"{"messages":[{"role":"system","content":"You are OpenClaw v1 with extra context injected each turn"},{"role":"user","content":"task"}],"tools":[{"type":"function","function":{"name":"exec"}}]}"#;
        let req1 = parse_request(body1);
        let req2 = parse_request(body2);
        let turn = TraceTurn {
            audit_id: "a1".into(),
            session_id: "s1".into(),
            agent_header: None,
            timestamp: Utc::now(),
            request_body: body1.to_vec(),
            response_body: vec![],
        };
        let ctx1 = resolve_agent(&turn, &req1, None);
        let ctx2 = resolve_agent(&turn, &req2, None);
        assert_eq!(ctx1.agent_id, ctx2.agent_id);
    }

    #[test]
    fn continues_recent_completed_run_in_same_session() {
        let req = parse_request(
            r#"{"messages":[{"role":"tool","content":"search results"}]}"#.as_bytes(),
        );
        let run = RunRecord {
            run_id: "r1".into(),
            agent_id: "a1".into(),
            session_id: "s1".into(),
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
            status: RunStatus::Completed,
            goal: "帮我调研珠海金智维，看看是否值得投资".into(),
            turn_count: 5,
            messages_seen: 10,
            graph_path: None,
        };
        assert!(!should_start_new_run(&req, Some(&run), Utc::now()));
    }

    #[test]
    fn infers_explore_agent_for_exec_only_openclaw() {
        let body = r#"{"messages":[{"role":"user","content":"Compare three HTTP client libraries"}],"tools":[{"type":"function","function":{"name":"exec"}}]}"#.as_bytes();
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
        assert_eq!(ctx.agent_record.display_name, "OpenClaw");
        assert_eq!(ctx.agent_record.agent_type, "explore");
    }

    #[test]
    fn goals_related_for_research_variants() {
        assert!(goals_related(
            "帮我调研一下珠海金智维，看看是否值得投资",
            "帮我调研珠海金智维融资情况",
        ));
        assert!(!goals_related("fix login bug", "调研珠海金智维"));
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
