use chrono::Utc;
use uuid::Uuid;
use xxhash_rust::xxh64::xxh64;

use crate::models::AgentRecord;
use crate::parser::ParsedRequest;

const AGENT_NS: Uuid = Uuid::NAMESPACE_OID;

pub struct AgentContext {
    pub agent_id: String,
    pub agent_record: AgentRecord,
    pub is_new_run: bool,
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

    let agent_type = infer_agent_type(&req.system_excerpt, &req.tools);
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

    let is_new_run = should_start_new_run(req, existing_agent);

    AgentContext {
        agent_id,
        agent_record: record,
        is_new_run,
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

fn infer_agent_type(system: &str, tools: &[String]) -> String {
    let lower = system.to_ascii_lowercase();
    let tool_str = tools.join(" ").to_ascii_lowercase();
    if lower.contains("claude code") || tool_str.contains("edit") && tool_str.contains("bash") {
        "coding".to_string()
    } else if tool_str.contains("browser") || tool_str.contains("search") || tool_str.contains("web") {
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

fn should_start_new_run(req: &ParsedRequest, _existing: Option<&AgentRecord>) -> bool {
    // First user message in this turn with no prior context signals a new task.
    req.new_messages
        .iter()
        .any(|m| m.role == "user" && !m.text.trim().is_empty())
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
}
