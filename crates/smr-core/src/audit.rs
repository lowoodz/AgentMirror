use chrono::{DateTime, Utc};
use serde::Serialize;
use smr_protocol::ApiProtocol;

#[derive(Debug, Clone, Serialize)]
pub struct RequestAudit {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub session_id: String,
    pub protocol: String,
    pub fallback_group: String,
    pub fallback_chain: Vec<String>,
    pub final_model: Option<String>,
    pub dlp_replacements: u32,
    pub safety_blocks: u32,
    pub safety_observations: u32,
    pub success: bool,
    pub message: String,
}

impl RequestAudit {
    pub fn summary(&self) -> String {
        format!(
            "Protocol: {} | Fallback: {} | DLP: {} | Safety: {} blocked, {} observed | {}",
            self.protocol,
            if self.fallback_chain.is_empty() {
                "none".into()
            } else {
                self.fallback_chain.join(" → ")
            },
            self.dlp_replacements,
            self.safety_blocks,
            self.safety_observations,
            if self.success { "OK" } else { "FAILED" }
        )
    }
}

pub fn protocol_label(p: ApiProtocol) -> &'static str {
    match p {
        ApiProtocol::OpenAi => "OpenAI",
        ApiProtocol::Anthropic => "Anthropic",
    }
}
