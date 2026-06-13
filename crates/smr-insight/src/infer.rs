use serde::Deserialize;

use crate::llm::LlmClient;
use crate::models::{
    CognitiveEvent, CounterfactualNote, DialecticalNotes, Issue, ReflectionReport, RunRecord,
    Suggestion,
};

const MAX_SUMMARY_CHARS: usize = 6000;

#[derive(Debug, Deserialize)]
struct LlmEnrichment {
    #[serde(default)]
    goal: Option<String>,
    #[serde(default)]
    issues: Vec<LlmIssue>,
    #[serde(default)]
    suggestions: Vec<LlmSuggestion>,
    #[serde(default)]
    dialectical: Option<LlmDialectical>,
    #[serde(default)]
    counterfactuals: Vec<LlmCounterfactual>,
    #[serde(default)]
    estimated_improvement: Option<String>,
    #[serde(default)]
    critics: Option<LlmCritics>,
}

#[derive(Debug, Deserialize)]
struct LlmIssue {
    message: String,
    #[serde(default = "default_severity")]
    severity: String,
}

#[derive(Debug, Deserialize)]
struct LlmSuggestion {
    message: String,
    #[serde(default)]
    rationale: String,
    #[serde(default = "default_priority")]
    priority: String,
}

#[derive(Debug, Deserialize)]
struct LlmDialectical {
    #[serde(default)]
    thesis: String,
    #[serde(default)]
    antithesis: Vec<String>,
    #[serde(default)]
    synthesis: String,
}

#[derive(Debug, Deserialize)]
struct LlmCounterfactual {
    decision: String,
    alternative: String,
    #[serde(default)]
    when_better: String,
}

#[derive(Debug, Deserialize)]
struct LlmCritics {
    #[serde(default)]
    alignment: u8,
    #[serde(default)]
    necessity: u8,
    #[serde(default)]
    completeness: u8,
    #[serde(default)]
    efficiency: u8,
    #[serde(default)]
    safety: u8,
}

fn default_severity() -> String {
    "medium".to_string()
}

fn default_priority() -> String {
    "medium".to_string()
}

pub fn maybe_llm_enrich(
    client: Option<&dyn LlmClient>,
    run: &RunRecord,
    events: &[CognitiveEvent],
    report: &mut ReflectionReport,
) {
    let Some(client) = client else {
        return;
    };
    let summary = compact_trajectory(events);
    if summary.is_empty() {
        return;
    }

    let system = "You are AgentMirror, an agent trajectory analyst. Respond with JSON only, no markdown fences.";
    let user = format!(
        r#"Analyze this agent task run and return JSON:
{{
  "goal": "refined one-line goal or null",
  "critics": {{ "alignment": 0-100, "necessity": 0-100, "completeness": 0-100, "efficiency": 0-100, "safety": 0-100 }},
  "issues": [{{ "message": "...", "severity": "low|medium|high" }}],
  "suggestions": [{{ "message": "...", "rationale": "...", "priority": "low|medium|high" }}],
  "dialectical": {{ "thesis": "what the agent did", "antithesis": ["alternative 1", "alternative 2"], "synthesis": "when original vs alternatives win" }},
  "counterfactuals": [{{ "decision": "...", "alternative": "...", "when_better": "..." }}],
  "estimated_improvement": "+N% or null"
}}

Current goal: {}
Turns: {}
Rule-based outcome: {:?}
Rule critics: alignment={} completeness={} efficiency={} safety={}

Trajectory:
{}"#,
        run.goal,
        run.turn_count,
        report.outcome,
        report.critics.alignment,
        report.critics.completeness,
        report.critics.efficiency,
        report.critics.safety,
        summary,
    );

    match client.complete(system, &user) {
        Ok(raw) => apply_enrichment(report, &raw),
        Err(err) => tracing::warn!(?err, run_id = %run.run_id, "AgentMirror LLM enrich failed"),
    }
}

fn apply_enrichment(report: &mut ReflectionReport, raw: &str) {
    let json_text = extract_json_object(raw);
    let Ok(parsed) = serde_json::from_str::<LlmEnrichment>(&json_text) else {
        tracing::warn!("AgentMirror LLM returned non-JSON");
        return;
    };

    if let Some(goal) = parsed.goal.filter(|g| !g.trim().is_empty()) {
        report.goal = goal;
    }
    if let Some(c) = parsed.critics {
        if c.alignment > 0 {
            report.critics.alignment = c.alignment;
        }
        if c.necessity > 0 {
            report.critics.necessity = c.necessity;
        }
        if c.completeness > 0 {
            report.critics.completeness = c.completeness;
        }
        if c.efficiency > 0 {
            report.critics.efficiency = c.efficiency;
        }
        if c.safety > 0 {
            report.critics.safety = c.safety;
        }
    }
    for issue in parsed.issues {
        if !report.issues.iter().any(|i| i.message == issue.message) {
            report.issues.push(Issue {
                message: issue.message,
                severity: issue.severity,
            });
        }
    }
    for sug in parsed.suggestions {
        if !report.suggestions.iter().any(|s| s.message == sug.message) {
            report.suggestions.push(Suggestion {
                message: sug.message,
                rationale: sug.rationale,
                priority: sug.priority,
            });
        }
    }
    if let Some(d) = parsed.dialectical {
        report.dialectical = Some(DialecticalNotes {
            thesis: d.thesis,
            antithesis: d.antithesis,
            synthesis: d.synthesis,
        });
    }
    if !parsed.counterfactuals.is_empty() {
        report.counterfactuals = parsed
            .counterfactuals
            .into_iter()
            .map(|c| CounterfactualNote {
                decision: c.decision,
                alternative: c.alternative,
                when_better: c.when_better,
            })
            .collect();
    }
    report.estimated_improvement = parsed.estimated_improvement;
}

fn compact_trajectory(events: &[CognitiveEvent]) -> String {
    let mut out = String::new();
    for event in events {
        let line = format!("[{}] {}\n", event.kind.as_str(), event.summary);
        if out.len() + line.len() > MAX_SUMMARY_CHARS {
            out.push_str("…(truncated)\n");
            break;
        }
        out.push_str(&line);
    }
    out
}

fn extract_json_object(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with('{') {
        return trimmed.to_string();
    }
    if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            return trimmed[start..=end].to_string();
        }
    }
    trimmed.to_string()
}

pub fn infer_goal_llm(
    client: Option<&dyn LlmClient>,
    events: &[CognitiveEvent],
    fallback: &str,
) -> String {
    let Some(client) = client else {
        return fallback.to_string();
    };
    let summary = compact_trajectory(events);
    if summary.is_empty() {
        return fallback.to_string();
    }
    let system = "Extract the agent task goal. Reply with JSON only: {\"goal\":\"...\",\"confidence\":0-1}";
    let user = format!("Trajectory:\n{summary}");
    match client.complete(system, &user) {
        Ok(raw) => {
            let json_text = extract_json_object(&raw);
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json_text) {
                if let Some(g) = v.get("goal").and_then(|x| x.as_str()) {
                    if !g.trim().is_empty() {
                        return g.trim().to_string();
                    }
                }
            }
            fallback.to_string()
        }
        Err(_) => fallback.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_json_from_fenced_response() {
        let raw = r#"Here is the analysis:
{"goal":"fix bug","critics":{"alignment":80,"necessity":70,"completeness":75,"efficiency":80,"safety":90}}
"#;
        assert!(extract_json_object(raw).starts_with('{'));
    }
}
