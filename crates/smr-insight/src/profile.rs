use crate::models::{AgentRecord, AgentRunStats};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentProfile {
    pub agent_id: String,
    pub display_name: String,
    pub agent_type: String,
    pub system_hash: String,
    pub tools: Vec<String>,
    pub capabilities: Vec<String>,
    pub stats: AgentRunStats,
    pub first_seen: chrono::DateTime<chrono::Utc>,
    pub last_seen: chrono::DateTime<chrono::Utc>,
}

pub fn build_profile(agent: &AgentRecord, stats: AgentRunStats) -> AgentProfile {
    let tools: Vec<String> = serde_json::from_str(&agent.tools_json).unwrap_or_default();
    let capabilities = infer_capabilities(&agent.agent_type, &tools);
    AgentProfile {
        agent_id: agent.agent_id.clone(),
        display_name: agent.display_name.clone(),
        agent_type: agent.agent_type.clone(),
        system_hash: agent.system_hash.clone(),
        tools,
        capabilities,
        stats,
        first_seen: agent.first_seen,
        last_seen: agent.last_seen,
    }
}

fn infer_capabilities(agent_type: &str, tools: &[String]) -> Vec<String> {
    let mut caps = Vec::new();
    let joined = tools.join(" ").to_ascii_lowercase();
    if agent_type == "coding" {
        caps.push("code_editing".to_string());
    }
    if joined.contains("bash") || joined.contains("shell") || joined.contains("terminal") {
        caps.push("shell_execution".to_string());
    }
    if joined.contains("edit") || joined.contains("write") || joined.contains("file") {
        caps.push("file_operations".to_string());
    }
    if joined.contains("browser") || joined.contains("search") || joined.contains("web") {
        caps.push("web_research".to_string());
    }
    if joined.contains("read") {
        caps.push("document_read".to_string());
    }
    if tools.is_empty() {
        caps.push("chat_only".to_string());
    } else if caps.is_empty() {
        caps.push("tool_use".to_string());
    }
    caps.sort();
    caps.dedup();
    caps
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn coding_agent_capabilities() {
        let agent = AgentRecord {
            agent_id: "a1".to_string(),
            display_name: "Coder".to_string(),
            agent_type: "coding".to_string(),
            system_hash: "abc".to_string(),
            tools_json: r#"["edit","bash","read"]"#.to_string(),
            first_seen: Utc::now(),
            last_seen: Utc::now(),
        };
        let profile = build_profile(
            &agent,
            AgentRunStats {
                total_runs: 5,
                completed: 4,
                failed: 1,
                running: 0,
                stale: 0,
                total_turns: 20,
                avg_turns: 4.0,
            },
        );
        assert!(profile.capabilities.contains(&"code_editing".to_string()));
        assert!(profile.capabilities.contains(&"shell_execution".to_string()));
    }
}
