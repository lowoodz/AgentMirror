use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Goal,
    SubGoal,
    Decision,
    Action,
    Observation,
    Reflection,
    Result,
    StateTransition,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Goal => "goal",
            Self::SubGoal => "sub_goal",
            Self::Decision => "decision",
            Self::Action => "action",
            Self::Observation => "observation",
            Self::Reflection => "reflection",
            Self::Result => "result",
            Self::StateTransition => "state_transition",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Completed,
    Failed,
    Stale,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Stale => "stale",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    Success,
    Partial,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRecord {
    pub agent_id: String,
    pub display_name: String,
    pub agent_type: String,
    pub system_hash: String,
    pub tools_json: String,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub run_id: String,
    pub agent_id: String,
    pub session_id: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub status: RunStatus,
    pub goal: String,
    pub turn_count: u32,
    /// Messages already processed from OpenAI-style `messages[]` history.
    pub messages_seen: u32,
    pub graph_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitiveEvent {
    pub id: String,
    pub run_id: String,
    pub seq: u32,
    pub kind: EventKind,
    pub timestamp: DateTime<Utc>,
    pub summary: String,
    pub audit_id: String,
    pub confidence: f32,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub kind: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningGraph {
    pub run_id: String,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CriticsScore {
    pub alignment: u8,
    pub necessity: u8,
    pub completeness: u8,
    pub efficiency: u8,
    pub safety: u8,
}

impl Default for CriticsScore {
    fn default() -> Self {
        Self {
            alignment: 70,
            necessity: 70,
            completeness: 70,
            efficiency: 70,
            safety: 90,
        }
    }
}

/// Narrative review for each of the five critic dimensions.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CriticsAnalysis {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub alignment: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub necessity: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub completeness: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub efficiency: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub safety: String,
}

impl CriticsAnalysis {
    pub fn any_populated(&self) -> bool {
        !self.alignment.is_empty()
            || !self.necessity.is_empty()
            || !self.completeness.is_empty()
            || !self.efficiency.is_empty()
            || !self.safety.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub message: String,
    pub severity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suggestion {
    pub message: String,
    pub rationale: String,
    pub priority: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DialecticalNotes {
    pub thesis: String,
    pub antithesis: Vec<String>,
    pub synthesis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CounterfactualNote {
    pub decision: String,
    pub alternative: String,
    pub when_better: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectionReport {
    pub run_id: String,
    /// Latest / current goal after iterative reflection (may differ from original).
    pub goal: String,
    /// True initial goal identified from the first events (LLM).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_goal: Option<String>,
    pub execution_summary: String,
    pub outcome: RunOutcome,
    pub issues: Vec<Issue>,
    pub risks: Vec<String>,
    pub suggestions: Vec<Suggestion>,
    pub critics: CriticsScore,
    #[serde(default)]
    pub critic_analyses: CriticsAnalysis,
    pub generated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialectical: Option<DialecticalNotes>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub counterfactuals: Vec<CounterfactualNote>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_improvement: Option<String>,
    /// LLM-produced logical critique (goal-action chain, gaps, reasoning quality).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logical_analysis: Option<String>,
    /// Short executive summary of the reflection (LLM when available).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reflection_summary: Option<String>,
    #[serde(default)]
    pub llm_enhanced: bool,
    /// Number of cognitive events included in the last LLM reflection pass.
    #[serde(default)]
    pub llm_event_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyReport {
    pub date: String,
    pub agent_id: String,
    pub display_name: String,
    pub summary: String,
    pub runs_completed: u32,
    pub runs_failed: u32,
    pub runs_running: u32,
    pub total_turns: u32,
    pub top_issues: Vec<String>,
    pub top_suggestions: Vec<String>,
    pub run_summaries: Vec<DailyRunSummary>,
    pub generated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyRunSummary {
    pub run_id: String,
    pub goal: String,
    pub status: String,
    pub turn_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunStats {
    pub total_runs: u32,
    pub completed: u32,
    pub failed: u32,
    pub running: u32,
    pub stale: u32,
    pub total_turns: u64,
    pub avg_turns: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunActionSequence {
    pub run_id: String,
    pub status: RunStatus,
    pub actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionPattern {
    pub steps: Vec<String>,
    pub success_count: u32,
    pub failure_count: u32,
    pub outcome_hint: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunRiskSummary {
    pub dlp_replacements: u32,
    pub safety_blocks: u32,
    pub safety_observations: u32,
    pub high_risk: bool,
}

#[derive(Debug, Clone)]
pub struct TraceTurn {
    pub audit_id: String,
    pub session_id: String,
    pub agent_header: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub request_body: Vec<u8>,
    pub response_body: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsightConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub require_traffic_bodies: bool,
    #[serde(default = "default_daily_hour")]
    pub daily_report_hour: u8,
    #[serde(default = "default_retention")]
    pub retention_days: u32,
    #[serde(default = "default_true")]
    pub llm_critic: bool,
    #[serde(default = "default_critic_group")]
    pub critic_model_group: String,
}

fn default_true() -> bool {
    true
}

fn default_daily_hour() -> u8 {
    8
}

fn default_retention() -> u32 {
    30
}

fn default_critic_group() -> String {
    "high".to_string()
}

impl Default for InsightConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            require_traffic_bodies: true,
            daily_report_hour: default_daily_hour(),
            retention_days: default_retention(),
            llm_critic: true,
            critic_model_group: default_critic_group(),
        }
    }
}

impl InsightConfig {
    pub fn normalize(&mut self) {
        if self.retention_days == 0 {
            self.retention_days = default_retention();
        }
        if self.daily_report_hour > 23 {
            self.daily_report_hour = default_daily_hour();
        }
    }
}
