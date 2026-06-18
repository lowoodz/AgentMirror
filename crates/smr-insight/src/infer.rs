use chrono::{NaiveDate, Utc};
use serde::Deserialize;

use crate::llm::LlmClient;
use crate::locale::ReportLanguage;
use crate::models::{
    CognitiveEvent, CounterfactualNote, CriticsAnalysis, CriticsScore, DailyReport, DialecticalNotes, Issue, ReflectionReport, RunOutcome, RunRecord,
    RunStatus, Suggestion,
};
use crate::token_budget::{batch_events, format_batch_trajectory, MAX_BATCH_TOKENS};

const INITIAL_GOAL_EVENT_COUNT: usize = 10;
const LLM_CRITIC_MAX_ATTEMPTS: u32 = 3;

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
    #[serde(default)]
    critic_analyses: Option<LlmCriticsAnalysis>,
    #[serde(default)]
    current_goal: Option<String>,
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

#[derive(Debug, Deserialize)]
struct LlmCriticsAnalysis {
    #[serde(default)]
    alignment: String,
    #[serde(default)]
    necessity: String,
    #[serde(default)]
    completeness: String,
    #[serde(default)]
    efficiency: String,
    #[serde(default)]
    safety: String,
}

fn default_severity() -> String {
    "medium".to_string()
}

fn default_priority() -> String {
    "medium".to_string()
}

const CRITIC_SYSTEM_TEMPLATE: &str = r#"You are AgentMirror Critic — an expert in agent trajectory analysis, formal logic, and dialectical reasoning.

You receive cognitive events in batches. When a prior reflection report is provided, MERGE and REFINE it with the new events — output one complete updated report covering everything seen so far, not just the latest batch.

Track goals across batches:
- **original_goal** is fixed (the user's initial intent).
- **current_goal** may evolve if the user shifts topic; update it when new events show a genuine goal change.
- Five-dimension alignment must judge actions against **current_goal**, noting drift from **original_goal** when they differ.

Method (apply all in every batch output):
1. **Logical critique**: Does the action chain support the stated goal? Missing steps? Non sequiturs?
2. **Dialectical analysis**: Thesis (what agent did), Antithesis (2–3 alternatives), Synthesis (when each wins).
3. **Counterfactuals**: For major Decisions in the trace seen so far, alternative paths and when better.
4. **Five critic dimensions** — score (0–100) AND 2–4 sentence analysis each:
   - alignment: goal + context vs actions — drift?
   - necessity: essential steps vs redundancy
   - completeness: thoroughness, missing phases
   - efficiency: direct path vs detours
   - safety: risks, dangerous tools, policy issues

{language_instruction}
Be specific — cite event seq numbers from the trace.
Respond with JSON only — no markdown fences."#;

fn critic_system_prompt(language: ReportLanguage) -> String {
    CRITIC_SYSTEM_TEMPLATE.replace(
        "{language_instruction}",
        language.write_instruction(),
    )
}

/// LLM-only reflection report: incremental batched event iteration.
pub fn generate_reflection_report_llm(
    client: Option<&dyn LlmClient>,
    run: &RunRecord,
    events: &[CognitiveEvent],
    execution_summary: &str,
    safety_notes: &[String],
    prior: Option<&ReflectionReport>,
    language: ReportLanguage,
) -> Option<ReflectionReport> {
    let client = client?;
    if events.is_empty() {
        return None;
    }

    let event_count = events.len() as u32;
    if let Some(p) = prior {
        if p.llm_enhanced && p.llm_event_count >= event_count {
            return Some(p.clone());
        }
    }

    let processed = prior.map(|p| p.llm_event_count).unwrap_or(0) as usize;
    let original_goal = prior
        .and_then(|p| p.original_goal.clone())
        .filter(|g| !g.trim().is_empty())
        .unwrap_or_else(|| infer_initial_goal_llm(client, events, &run.goal, language));
    let mut current_goal = prior
        .map(|p| p.goal.clone())
        .unwrap_or_else(|| original_goal.clone());

    let skip = if processed > 0 && processed < events.len() {
        processed
    } else if processed >= events.len() {
        return prior.cloned();
    } else {
        0
    };
    let events_slice = &events[skip..];
    if events_slice.is_empty() {
        return prior.cloned();
    }

    let batches = batch_events(events_slice, MAX_BATCH_TOKENS);
    let total_batches = batches.len();
    let mut previous_report_json = prior.and_then(|p| {
        if p.llm_enhanced {
            serde_json::to_string(p).ok()
        } else {
            None
        }
    });
    let mut final_report: Option<ReflectionReport> = None;

    for (batch_idx, batch) in batches.iter().enumerate() {
        let batch_num = batch_idx + 1;
        let trajectory = format_batch_trajectory(batch);
        let user = build_batched_critic_prompt(
            run,
            execution_summary,
            safety_notes,
            &original_goal,
            &current_goal,
            batch_num,
            total_batches,
            previous_report_json.as_deref(),
            &trajectory,
        );

        let mut batch_done = false;
        for attempt in 0..LLM_CRITIC_MAX_ATTEMPTS {
            match client.complete(&critic_system_prompt(language), &user) {
                Ok(raw) => {
                    let mut report = report_shell(
                        run,
                        execution_summary,
                        &original_goal,
                        &current_goal,
                        safety_notes,
                    );
                    if let Some(p) = prior {
                        if p.llm_enhanced && batch_idx == 0 {
                            report = p.clone();
                            report.execution_summary = execution_summary.to_string();
                        }
                    }
                    if apply_llm_critic(&mut report, &raw, true, &original_goal) {
                        report.llm_event_count = event_count;
                        report.generated_at = Utc::now();
                        current_goal = report.goal.clone();
                        previous_report_json = Some(extract_json_object(&raw));
                        final_report = Some(report);
                        tracing::debug!(
                            run_id = %run.run_id,
                            batch = batch_num,
                            total = total_batches,
                            events = event_count,
                            "AgentMirror LLM critic batch complete"
                        );
                    } else {
                        tracing::warn!(
                            run_id = %run.run_id,
                            batch = batch_num,
                            "AgentMirror LLM critic batch returned invalid JSON"
                        );
                    }
                    batch_done = true;
                    break;
                }
                Err(err) => {
                    if attempt + 1 < LLM_CRITIC_MAX_ATTEMPTS {
                        let delay_ms = 500u64 * 2u64.pow(attempt);
                        tracing::warn!(
                            ?err,
                            run_id = %run.run_id,
                            batch = batch_num,
                            attempt = attempt + 1,
                            delay_ms,
                            "AgentMirror LLM critic batch failed — retrying"
                        );
                        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                        continue;
                    }
                    tracing::warn!(
                        ?err,
                        run_id = %run.run_id,
                        batch = batch_num,
                        "AgentMirror LLM critic batch failed"
                    );
                    break;
                }
            }
        }
        if !batch_done {
            break;
        }
    }

    if final_report.is_none() {
        prior.cloned()
    } else {
        final_report
    }
}

fn report_shell(
    run: &RunRecord,
    execution_summary: &str,
    original_goal: &str,
    current_goal: &str,
    safety_notes: &[String],
) -> ReflectionReport {
    let mut report = ReflectionReport {
        run_id: run.run_id.clone(),
        goal: current_goal.to_string(),
        original_goal: Some(original_goal.to_string()),
        execution_summary: execution_summary.to_string(),
        outcome: outcome_from_run_status(run.status),
        issues: Vec::new(),
        risks: Vec::new(),
        suggestions: Vec::new(),
        critics: CriticsScore::default(),
        critic_analyses: CriticsAnalysis::default(),
        generated_at: Utc::now(),
        dialectical: None,
        counterfactuals: Vec::new(),
        estimated_improvement: None,
        logical_analysis: None,
        reflection_summary: None,
        llm_enhanced: false,
        llm_event_count: 0,
        llm_turn_count: 0,
        llm_run_status: None,
    };
    merge_safety_issues(&mut report, safety_notes);
    report
}

fn merge_safety_issues(report: &mut ReflectionReport, safety_notes: &[String]) {
    for finding in safety_notes {
        let issue = Issue {
            message: finding.clone(),
            severity: "high".to_string(),
        };
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
}

fn outcome_from_run_status(status: RunStatus) -> RunOutcome {
    match status {
        RunStatus::Completed => RunOutcome::Success,
        RunStatus::Failed => RunOutcome::Failed,
        RunStatus::Running | RunStatus::Stale => RunOutcome::Partial,
    }
}

fn build_batched_critic_prompt(
    run: &RunRecord,
    execution_summary: &str,
    safety_notes: &[String],
    original_goal: &str,
    current_goal: &str,
    batch_num: usize,
    total_batches: usize,
    previous_report_json: Option<&str>,
    batch_trajectory: &str,
) -> String {
    let safety = if safety_notes.is_empty() {
        "none".to_string()
    } else {
        safety_notes.join("; ")
    };
    let prior = previous_report_json.unwrap_or("(none — first batch)");

    format!(
        r#"Return JSON with this exact schema:
{{
  "original_goal": "unchanged initial goal (echo input unless correcting a clear error)",
  "current_goal": "latest goal after this batch; echo prior current_goal if unchanged",
  "reflection_summary": "2-4 sentence executive summary",
  "logical_analysis": "paragraph: logical structure goal→actions→outcome",
  "critics": {{ "alignment": 0-100, "necessity": 0-100, "completeness": 0-100, "efficiency": 0-100, "safety": 0-100 }},
  "critic_analyses": {{
    "alignment": "2-4 sentences with event refs; note drift from original_goal if any",
    "necessity": "2-4 sentences",
    "completeness": "2-4 sentences",
    "efficiency": "2-4 sentences",
    "safety": "2-4 sentences"
  }},
  "issues": [{{ "message": "...", "severity": "low|medium|high" }}],
  "suggestions": [{{ "message": "...", "rationale": "...", "priority": "low|medium|high", "related_event_seq": null or number }}],
  "dialectical": {{ "thesis": "...", "antithesis": ["...", "..."], "synthesis": "..." }},
  "counterfactuals": [{{ "decision": "...", "alternative": "...", "when_better": "..." }}],
  "estimated_improvement": "+N% or null"
}}

Run context:
- run_id: {}
- status: {}
- turns: {}
- original_goal (fixed): {}
- current_goal (may evolve): {}
- run record goal hint: {}
- action chain summary: {}
- safety / policy notes: {}

Batch {}/{} — events in THIS message (seq, kind, summary):
{}

Prior reflection report JSON (merge original_goal, current_goal, and all fields; cover ALL events seen so far):
{}
"#,
        run.run_id,
        run.status.as_str(),
        run.turn_count,
        original_goal,
        current_goal,
        run.goal,
        execution_summary,
        safety,
        batch_num,
        total_batches,
        batch_trajectory,
        prior,
    )
}

fn apply_llm_critic(
    report: &mut ReflectionReport,
    raw: &str,
    preserve_rule_safety: bool,
    original_goal: &str,
) -> bool {
    let json_text = extract_json_object(raw);
    let Ok(parsed) = serde_json::from_str::<LlmCriticResponse>(&json_text) else {
        tracing::warn!("AgentMirror LLM critic returned non-JSON");
        return false;
    };

    report.original_goal = Some(original_goal.to_string());

    let latest_goal = parsed
        .current_goal
        .filter(|g| !g.trim().is_empty())
        .or(parsed.goal.filter(|g| !g.trim().is_empty()));
    if let Some(goal) = latest_goal {
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
    if let Some(a) = parsed.critic_analyses {
        apply_critic_analyses(&mut report.critic_analyses, &a);
    }

    let rule_safety: Vec<Issue> = if preserve_rule_safety {
        report
            .issues
            .iter()
            .filter(|i| i.severity == "high")
            .cloned()
            .collect()
    } else {
        Vec::new()
    };

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
        || report.dialectical.is_some()
        || report.critic_analyses.any_populated();

    report.llm_enhanced
}

fn apply_critic_analyses(target: &mut CriticsAnalysis, src: &LlmCriticsAnalysis) {
    if !src.alignment.trim().is_empty() {
        target.alignment = src.alignment.trim().to_string();
    }
    if !src.necessity.trim().is_empty() {
        target.necessity = src.necessity.trim().to_string();
    }
    if !src.completeness.trim().is_empty() {
        target.completeness = src.completeness.trim().to_string();
    }
    if !src.efficiency.trim().is_empty() {
        target.efficiency = src.efficiency.trim().to_string();
    }
    if !src.safety.trim().is_empty() {
        target.safety = src.safety.trim().to_string();
    }
}

fn apply_critic_scores(target: &mut CriticsScore, c: &LlmCritics) {
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

pub fn infer_initial_goal_llm(
    client: &dyn LlmClient,
    events: &[CognitiveEvent],
    fallback: &str,
    language: ReportLanguage,
) -> String {
    let head: Vec<&CognitiveEvent> = events.iter().take(INITIAL_GOAL_EVENT_COUNT).collect();
    let trajectory = format_batch_trajectory(&head);
    if trajectory.is_empty() {
        return fallback.to_string();
    }
    let system = format!(
        r#"Identify the user's true initial task goal from the FIRST events of an agent run.
Reply with JSON only: {{"original_goal":"one-line goal","confidence":0-1}}
{}"#,
        language.write_instruction()
    );
    let user = format!(
        "First {} event(s) of the run:\n{trajectory}",
        head.len()
    );
    match client.complete(&system, &user) {
        Ok(raw) => {
            let json_text = extract_json_object(&raw);
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json_text) {
                for key in ["original_goal", "goal"] {
                    if let Some(g) = v.get(key).and_then(|x| x.as_str()) {
                        if !g.trim().is_empty() {
                            return g.trim().to_string();
                        }
                    }
                }
            }
            fallback.to_string()
        }
        Err(err) => {
            tracing::warn!(?err, "AgentMirror initial goal LLM failed");
            fallback.to_string()
        }
    }
}

#[derive(Debug, Deserialize)]
struct LlmDailyResponse {
    #[serde(default)]
    task_progress: Vec<crate::models::DailyTaskProgress>,
    #[serde(default)]
    daily_issues: Vec<crate::models::DailyIssueItem>,
    #[serde(default)]
    top_suggestions: Vec<String>,
}

const DAILY_SYSTEM_TEMPLATE: &str = r#"You are AgentMirror Daily Analyst — synthesize one executive daily report across ALL agents (智能体) for a single calendar day.

You receive per-run reflection reports and run metadata. Produce JSON with:
1. **task_progress** — ordered list of tasks for the day. Each item: run_id, goal (concise), status (completed|running|failed), turn_count, duration_minutes (optional).
2. **daily_issues** — up to 6 cross-run problems/risks. Each item: run_id, message, dimension (Alignment|Necessity|Completeness|Efficiency|Safety), score (0-100).
3. **top_suggestions** — up to 6 actionable recommendations for tomorrow (strings).

{language_instruction}
Respond with JSON only — no markdown fences. Schema:
{"task_progress":[{"run_id":"...","goal":"...","status":"completed","turn_count":12,"duration_minutes":23}],"daily_issues":[{"run_id":"...","message":"...","dimension":"Completeness","score":62}],"top_suggestions":["..."]}"#;

fn daily_system_prompt(language: ReportLanguage) -> String {
    DAILY_SYSTEM_TEMPLATE.replace(
        "{language_instruction}",
        language.write_instruction(),
    )
}

/// Generate a daily report via LLM using per-run reflection inputs.
pub fn generate_daily_report_llm(
    client: &dyn LlmClient,
    date: NaiveDate,
    baseline: &DailyReport,
    run_inputs: &[DailyRunReflectionInput],
    language: ReportLanguage,
) -> Option<DailyReport> {
    let mut user = format!(
        "Date: {}\nStats: {} runs — {} completed, {} running, {} failed, {} turns total.\n\nPer-run reflection inputs:\n",
        date,
        baseline.run_summaries.len(),
        baseline.runs_completed,
        baseline.runs_running,
        baseline.runs_failed,
        baseline.total_turns,
    );
    for (idx, input) in run_inputs.iter().take(80).enumerate() {
        user.push_str(&format!(
            "\n--- Run {} ({}) ---\nAgent: {} ({})\nGoal: {}\nStatus: {} · {} turns",
            idx + 1,
            input.run_id,
            input.display_name,
            input.agent_id,
            input.goal,
            input.status,
            input.turn_count,
        ));
        if let Some(mins) = input.duration_minutes {
            user.push_str(&format!(" · {mins} min"));
        }
        user.push('\n');
        if let Some(summary) = &input.reflection_summary {
            user.push_str(&format!("Reflection summary: {summary}\n"));
        }
        if !input.issues.is_empty() {
            user.push_str(&format!("Issues: {}\n", input.issues.join("; ")));
        }
        if !input.suggestions.is_empty() {
            user.push_str(&format!("Suggestions: {}\n", input.suggestions.join("; ")));
        }
    }
    if run_inputs.len() > 80 {
        user.push_str(&format!(
            "\n(... {} additional runs omitted for length)\n",
            run_inputs.len() - 80
        ));
    }

    let raw = client.complete(&daily_system_prompt(language), &user).ok()?;
    let json_text = extract_json_object(&raw);
    let parsed: LlmDailyResponse = serde_json::from_str(&json_text).ok()?;

    let mut report = baseline.clone();
    let has_tasks = !parsed.task_progress.is_empty();
    let has_issues = !parsed.daily_issues.is_empty();
    let has_suggestions = !parsed.top_suggestions.is_empty();
    if has_tasks {
        report.task_progress = parsed.task_progress;
    }
    if has_issues {
        report.daily_issues = parsed.daily_issues;
        report.daily_issues.truncate(6);
        report.top_issues = report
            .daily_issues
            .iter()
            .map(|i| {
                format!(
                    "Run #{}: {}{}",
                    crate::report::run_short_id(&i.run_id),
                    i.message,
                    match (&i.dimension, i.score) {
                        (Some(dim), Some(score)) => format!(" ({dim} {score})"),
                        _ => String::new(),
                    }
                )
            })
            .collect();
    }
    if has_suggestions {
        report.top_suggestions = parsed.top_suggestions;
        report.top_suggestions.truncate(6);
    }
    report.llm_enhanced = has_tasks || has_issues || has_suggestions;
    Some(report)
}

/// Per-run slice fed into daily LLM synthesis.
pub struct DailyRunReflectionInput {
    pub run_id: String,
    pub agent_id: String,
    pub display_name: String,
    pub goal: String,
    pub status: String,
    pub turn_count: u32,
    pub duration_minutes: Option<u32>,
    pub reflection_summary: Option<String>,
    pub issues: Vec<String>,
    pub suggestions: Vec<String>,
}

/// Legacy helper — uses first events only (same as reflection pipeline step 1).
pub fn infer_goal_llm(
    client: Option<&dyn LlmClient>,
    events: &[CognitiveEvent],
    fallback: &str,
    language: ReportLanguage,
) -> String {
    let Some(client) = client else {
        return fallback.to_string();
    };
    infer_initial_goal_llm(client, events, fallback, language)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::EventKind;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct BatchMockLlm {
        calls: Arc<AtomicUsize>,
        response: String,
    }

    impl LlmClient for BatchMockLlm {
        fn complete(&self, _system: &str, user: &str) -> anyhow::Result<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if user.contains("Batch") {
                Ok(self.response.clone())
            } else if user.contains("First ") && user.contains("event(s)") {
                Ok(r#"{"original_goal":"Fix login timeout","confidence":0.9}"#.to_string())
            } else {
                Ok(self.response.clone())
            }
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

    const LLM_JSON: &str = r#"{
        "original_goal": "Fix login timeout",
        "current_goal": "Fix login timeout",
        "reflection_summary": "Agent gathered logs but skipped verification.",
        "logical_analysis": "Reading logs supports diagnosis but no test validates the fix.",
        "critics": {"alignment": 82, "necessity": 75, "completeness": 60, "efficiency": 70, "safety": 95},
        "critic_analyses": {
            "alignment": "Actions focus on auth logs which match the login timeout goal.",
            "necessity": "Log reading was necessary; no redundant retries.",
            "completeness": "Missing verification after the patch.",
            "efficiency": "Three turns is reasonable for this scope.",
            "safety": "No destructive commands detected."
        },
        "issues": [{"message": "No verification step", "severity": "medium"}],
        "suggestions": [{"message": "Run integration test", "rationale": "Confirm fix", "priority": "high"}],
        "dialectical": {
            "thesis": "Read logs then patch auth module",
            "antithesis": ["Reproduce with minimal test first", "Check metrics dashboard"],
            "synthesis": "Reproduction first when bug is intermittent"
        },
        "counterfactuals": [{"decision": "I'll read the auth logs first", "alternative": "Write failing test", "when_better": "When bug is reproducible"}]
    }"#;

    #[test]
    fn extracts_json_from_fenced_response() {
        let raw = r#"Here is the analysis:
{"goal":"fix bug","critics":{"alignment":80,"necessity":70,"completeness":75,"efficiency":80,"safety":90}}
"#;
        assert!(extract_json_object(raw).starts_with('{'));
    }

    #[test]
    fn llm_only_report_from_batched_pipeline() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mock = BatchMockLlm {
            calls: Arc::clone(&calls),
            response: LLM_JSON.to_string(),
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
        let report = generate_reflection_report_llm(
            Some(&mock),
            &run,
            &sample_events(),
            "Read auth logs",
            &[],
            None,
            ReportLanguage::En,
        )
        .expect("report");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(report.llm_enhanced);
        assert_eq!(report.original_goal.as_deref(), Some("Fix login timeout"));
        assert_eq!(report.goal, "Fix login timeout");
        assert!(report.dialectical.is_some());
        assert_eq!(report.critics.completeness, 60);
        assert!(report.critic_analyses.completeness.contains("verification"));
    }

    #[test]
    fn multi_batch_calls_llm_per_batch() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mock = BatchMockLlm {
            calls: Arc::clone(&calls),
            response: LLM_JSON.to_string(),
        };
        let run = RunRecord {
            run_id: "r1".into(),
            agent_id: "a1".into(),
            session_id: "s1".into(),
            started_at: Utc::now(),
            ended_at: None,
            status: RunStatus::Running,
            goal: "Research task".into(),
            turn_count: 100,
            messages_seen: 0,
            graph_path: None,
        };
        let mut events = Vec::new();
        let long_summary = "查".repeat(600);
        for i in 0..200 {
            events.push(CognitiveEvent {
                id: format!("e{i}"),
                run_id: "r1".into(),
                seq: i,
                kind: EventKind::Action,
                timestamp: Utc::now(),
                summary: format!("{long_summary} step {i}"),
                audit_id: "a1".into(),
                confidence: 1.0,
                metadata: serde_json::Value::Null,
            });
        }
        let report = generate_reflection_report_llm(
            Some(&mock),
            &run,
            &events,
            "many steps",
            &[],
            None,
            ReportLanguage::En,
        )
        .expect("report");
        assert!(calls.load(Ordering::SeqCst) > 2);
        assert!(report.llm_enhanced);
    }
}
