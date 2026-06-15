use serde::Deserialize;

use crate::llm::LlmClient;
use crate::models::{
    CognitiveEvent, CounterfactualNote, DialecticalNotes, EventKind, Issue, ReflectionReport,
    RunRecord, Suggestion,
};

const MAX_TRAJECTORY_CHARS: usize = 10_000;

#[derive(Debug, Deserialize)]
struct LlmCriticResponse {
    #[serde(default)]
    goal: Option<String>,
    #[serde(default)]
    reflection_summary: Option<String>,
    #[serde(default)]
    logical_analysis: Option<String>,
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
    #[serde(default)]
    related_event_seq: Option<u32>,
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

const CRITIC_SYSTEM: &str = r#"You are AgentMirror Critic — an expert in agent trajectory analysis, formal logic, and dialectical reasoning.

Your job: critically review an autonomous agent's task run from its cognitive event trace (Goal → Decision → Action → Observation → Result).

Method (apply all):
1. **Logical critique**: Does the action chain support the stated goal? Missing steps? Non sequiturs? Premature conclusions?
2. **Dialectical analysis** (Hegelian structure):
   - Thesis: what the agent actually did and why (concrete, cite decisions/actions).
   - Antithesis: 2–3 credible alternative strategies the agent could have taken instead.
   - Synthesis: under which conditions the agent's path vs each alternative is preferable.
3. **Counterfactuals**: For each major Decision in the trace, one alternative path and when it would be better.
4. **Five critics** (0–100, justify via issues/suggestions):
   - alignment (goal ↔ actions), necessity (redundancy), completeness (missing phases), efficiency (turns/steps), safety (risky tools).

Write in the same language as the user's goal (Chinese goal → Chinese output; English → English).
Be specific and actionable — avoid generic advice. Reference concrete events from the trace.
Respond with JSON only — no markdown fences, no commentary outside JSON."#;

/// Run LLM dialectical + logical critic; returns true when the report was enriched.
pub fn enrich_report_with_llm(
    client: Option<&dyn LlmClient>,
    run: &RunRecord,
    events: &[CognitiveEvent],
    report: &mut ReflectionReport,
) -> bool {
    let Some(client) = client else {
        return false;
    };
    if events.len() < 2 {
        return false;
    }
    let trajectory = format_trajectory(events);
    if trajectory.is_empty() {
        return false;
    }

    let user = build_critic_user_prompt(run, report, events, &trajectory);
    match client.complete(CRITIC_SYSTEM, &user) {
        Ok(raw) => apply_llm_critic(report, &raw),
        Err(err) => {
            tracing::warn!(?err, run_id = %run.run_id, "AgentMirror LLM critic failed");
            false
        }
    }
}

fn build_critic_user_prompt(
    run: &RunRecord,
    report: &ReflectionReport,
    events: &[CognitiveEvent],
    trajectory: &str,
) -> String {
    let decision_lines: Vec<String> = events
        .iter()
        .filter(|e| e.kind == EventKind::Decision)
        .map(|e| format!("  seq {}: {}", e.seq, e.summary))
        .collect();
    let safety_notes = if report.risks.is_empty() {
        "none".to_string()
    } else {
        report.risks.join("; ")
    };

    format!(
        r#"Return JSON with this exact schema:
{{
  "goal": "one-line refined goal or null",
  "reflection_summary": "2-4 sentence executive summary for a human reviewer",
  "logical_analysis": "paragraph: logical structure of goal→actions→outcome, gaps, invalid leaps",
  "critics": {{ "alignment": 0-100, "necessity": 0-100, "completeness": 0-100, "efficiency": 0-100, "safety": 0-100 }},
  "issues": [{{ "message": "specific finding", "severity": "low|medium|high" }}],
  "suggestions": [{{ "message": "actionable improvement", "rationale": "why", "priority": "low|medium|high", "related_event_seq": null or number }}],
  "dialectical": {{
    "thesis": "what the agent did (concrete)",
    "antithesis": ["alternative strategy 1", "alternative strategy 2"],
    "synthesis": "when thesis vs alternatives win"
  }},
  "counterfactuals": [{{ "decision": "quoted decision", "alternative": "what to do instead", "when_better": "condition" }}],
  "estimated_improvement": "+N% efficiency or null"
}}

Run context:
- run_id: {}
- status: {}
- turns: {}
- stated goal: {}
- rule-based outcome: {:?}
- rule-based scores: alignment={} necessity={} completeness={} efficiency={} safety={}
- action chain summary: {}
- safety / policy notes: {}
- decision events:
{}

Cognitive event trace (seq, kind, summary):
{}
"#,
        run.run_id,
        run.status.as_str(),
        run.turn_count,
        run.goal,
        report.outcome,
        report.critics.alignment,
        report.critics.necessity,
        report.critics.completeness,
        report.critics.efficiency,
        report.critics.safety,
        report.execution_summary,
        safety_notes,
        if decision_lines.is_empty() {
            "  (none extracted)".to_string()
        } else {
            decision_lines.join("\n")
        },
        trajectory,
    )
}

fn apply_llm_critic(report: &mut ReflectionReport, raw: &str) -> bool {
    let json_text = extract_json_object(raw);
    let Ok(parsed) = serde_json::from_str::<LlmCriticResponse>(&json_text) else {
        tracing::warn!("AgentMirror LLM critic returned non-JSON");
        return false;
    };

    if let Some(goal) = parsed.goal.filter(|g| !g.trim().is_empty()) {
        report.goal = goal;
    }
    report.reflection_summary = parsed
        .reflection_summary
        .filter(|s| !s.trim().is_empty());
    report.logical_analysis = parsed
        .logical_analysis
        .filter(|s| !s.trim().is_empty());

    if let Some(c) = parsed.critics {
        apply_critic_scores(&mut report.critics, &c);
    }

    let rule_safety: Vec<Issue> = report
        .issues
        .iter()
        .filter(|i| i.severity == "high")
        .cloned()
        .collect();

    if !parsed.issues.is_empty() {
        report.issues = parsed
            .issues
            .into_iter()
            .map(|i| Issue {
                message: i.message,
                severity: i.severity,
            })
            .collect();
    }
    for issue in rule_safety {
        if !report.issues.iter().any(|i| i.message == issue.message) {
            report.issues.push(issue);
        }
    }
    report.risks = report
        .issues
        .iter()
        .filter(|i| i.severity == "high")
        .map(|i| i.message.clone())
        .collect();

    if !parsed.suggestions.is_empty() {
        report.suggestions = parsed
            .suggestions
            .into_iter()
            .map(|s| {
                let mut rationale = s.rationale;
                if let Some(seq) = s.related_event_seq {
                    if !rationale.is_empty() {
                        rationale.push_str(&format!(" (event #{seq})"));
                    } else {
                        rationale = format!("Related to event #{seq}");
                    }
                }
                Suggestion {
                    message: s.message,
                    rationale,
                    priority: s.priority,
                }
            })
            .collect();
    }

    if let Some(d) = parsed.dialectical {
        if !d.thesis.trim().is_empty() || !d.synthesis.trim().is_empty() {
            report.dialectical = Some(DialecticalNotes {
                thesis: d.thesis,
                antithesis: d.antithesis,
                synthesis: d.synthesis,
            });
        }
    }
    if !parsed.counterfactuals.is_empty() {
        report.counterfactuals = parsed
            .counterfactuals
            .into_iter()
            .filter(|c| !c.decision.trim().is_empty())
            .map(|c| CounterfactualNote {
                decision: c.decision,
                alternative: c.alternative,
                when_better: c.when_better,
            })
            .collect();
    }
    report.estimated_improvement = parsed.estimated_improvement;
    report.llm_enhanced = report.reflection_summary.is_some()
        || report.logical_analysis.is_some()
        || report.dialectical.is_some();

    report.llm_enhanced
}

fn apply_critic_scores(target: &mut crate::models::CriticsScore, c: &LlmCritics) {
    if c.alignment > 0 {
        target.alignment = c.alignment;
    }
    if c.necessity > 0 {
        target.necessity = c.necessity;
    }
    if c.completeness > 0 {
        target.completeness = c.completeness;
    }
    if c.efficiency > 0 {
        target.efficiency = c.efficiency;
    }
    if c.safety > 0 {
        target.safety = c.safety;
    }
}

fn format_trajectory(events: &[CognitiveEvent]) -> String {
    let mut out = String::new();
    for event in events {
        let line = format!("#{} [{}] {}\n", event.seq, event.kind.as_str(), event.summary);
        if out.len() + line.len() > MAX_TRAJECTORY_CHARS {
            out.push_str("…(trajectory truncated)\n");
            break;
        }
        out.push_str(&line);
    }
    out
}

pub fn extract_json_object(raw: &str) -> String {
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
    let summary = format_trajectory(events);
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
    use crate::models::{CriticsScore, RunOutcome, RunStatus};
    use chrono::Utc;

    struct MockLlm {
        response: String,
    }

    impl LlmClient for MockLlm {
        fn complete(&self, _system: &str, _user: &str) -> anyhow::Result<String> {
            Ok(self.response.clone())
        }
    }

    fn sample_events() -> Vec<CognitiveEvent> {
        vec![
            CognitiveEvent {
                id: "e1".into(),
                run_id: "r1".into(),
                seq: 0,
                kind: EventKind::Goal,
                timestamp: Utc::now(),
                summary: "Fix login timeout".into(),
                audit_id: "a1".into(),
                confidence: 1.0,
                metadata: serde_json::Value::Null,
            },
            CognitiveEvent {
                id: "e2".into(),
                run_id: "r1".into(),
                seq: 1,
                kind: EventKind::Decision,
                timestamp: Utc::now(),
                summary: "I'll read the auth logs first".into(),
                audit_id: "a1".into(),
                confidence: 0.8,
                metadata: serde_json::Value::Null,
            },
            CognitiveEvent {
                id: "e3".into(),
                run_id: "r1".into(),
                seq: 2,
                kind: EventKind::Action,
                timestamp: Utc::now(),
                summary: "Read(/var/log/auth.log)".into(),
                audit_id: "a1".into(),
                confidence: 1.0,
                metadata: serde_json::Value::Null,
            },
        ]
    }

    #[test]
    fn extracts_json_from_fenced_response() {
        let raw = r#"Here is the analysis:
{"goal":"fix bug","critics":{"alignment":80,"necessity":70,"completeness":75,"efficiency":80,"safety":90}}
"#;
        assert!(extract_json_object(raw).starts_with('{'));
    }

    #[test]
    fn llm_critic_enriches_dialectical_fields() {
        let llm_json = r#"{
            "reflection_summary": "Agent gathered logs but skipped verification.",
            "logical_analysis": "Reading logs supports diagnosis but no test validates the fix.",
            "critics": {"alignment": 82, "necessity": 75, "completeness": 60, "efficiency": 70, "safety": 95},
            "issues": [{"message": "No verification step", "severity": "medium"}],
            "suggestions": [{"message": "Run integration test", "rationale": "Confirm fix", "priority": "high"}],
            "dialectical": {
                "thesis": "Read logs then patch auth module",
                "antithesis": ["Reproduce with minimal test first", "Check metrics dashboard"],
                "synthesis": "Reproduction first when bug is intermittent"
            },
            "counterfactuals": [{"decision": "I'll read the auth logs first", "alternative": "Write failing test", "when_better": "When bug is reproducible"}]
        }"#;
        let mock = MockLlm {
            response: llm_json.to_string(),
        };
        let run = RunRecord {
            run_id: "r1".into(),
            agent_id: "a1".into(),
            session_id: "s1".into(),
            started_at: Utc::now(),
            ended_at: None,
            status: RunStatus::Running,
            goal: "Fix login timeout".into(),
            turn_count: 3,
            messages_seen: 0,
            graph_path: None,
        };
        let events = sample_events();
        let mut report = ReflectionReport {
            run_id: "r1".into(),
            goal: run.goal.clone(),
            execution_summary: "Read".into(),
            outcome: RunOutcome::Partial,
            issues: vec![],
            risks: vec![],
            suggestions: vec![],
            critics: CriticsScore::default(),
            generated_at: Utc::now(),
            dialectical: None,
            counterfactuals: vec![],
            estimated_improvement: None,
            logical_analysis: None,
            reflection_summary: None,
            llm_enhanced: false,
        };
        assert!(enrich_report_with_llm(Some(&mock), &run, &events, &mut report));
        assert!(report.llm_enhanced);
        assert!(report.dialectical.is_some());
        assert_eq!(report.critics.completeness, 60);
        assert!(!report.counterfactuals.is_empty());
    }
}
