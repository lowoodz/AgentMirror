use chrono::{NaiveDate, Utc};

use crate::critic::{evaluate, CriticInput};
use crate::graph::{build_graph, execution_summary};
use crate::models::{
    DailyReport, DailyRunSummary, ReflectionReport, RunOutcome, RunRecord, RunStatus,
};
use crate::infer::maybe_llm_enrich;
use crate::llm::LlmClient;
use crate::safety::{scan_action_events, SafetyScanner};
use crate::store::InsightStore;

pub fn build_reflection_report(
    store: &InsightStore,
    run: &RunRecord,
    safety: Option<&dyn SafetyScanner>,
    llm: Option<&dyn LlmClient>,
    llm_critic: bool,
) -> anyhow::Result<ReflectionReport> {
    let events = store.list_events(&run.run_id)?;
    let safety_findings = scan_action_events(&events, safety);
    let (critics, issues, suggestions, outcome) = evaluate(CriticInput {
        events: &events,
        turn_count: run.turn_count,
        goal: &run.goal,
        safety_findings: &safety_findings,
    });

    let summary = execution_summary(&events);
    let execution_summary = if summary.is_empty() {
        "No actions recorded yet".to_string()
    } else {
        summary
    };

    let risks = issues
        .iter()
        .filter(|i| i.severity == "high")
        .map(|i| i.message.clone())
        .collect();

    let mut report = ReflectionReport {
        run_id: run.run_id.clone(),
        goal: run.goal.clone(),
        execution_summary,
        outcome,
        issues,
        risks,
        suggestions,
        critics,
        generated_at: Utc::now(),
        dialectical: None,
        counterfactuals: Vec::new(),
        estimated_improvement: None,
    };

    if llm_critic && run.status != RunStatus::Running {
        maybe_llm_enrich(llm, run, &events, &mut report);
    }

    Ok(report)
}

pub fn generate_daily_report(
    store: &InsightStore,
    agent_id: &str,
    date: NaiveDate,
) -> anyhow::Result<Option<DailyReport>> {
    let agent = match store.get_agent(agent_id)? {
        Some(a) => a,
        None => return Ok(None),
    };
    let runs = store.runs_for_agent_on_date(agent_id, date)?;
    if runs.is_empty() {
        return Ok(None);
    }

    let mut runs_completed = 0u32;
    let mut runs_failed = 0u32;
    let mut runs_running = 0u32;
    let mut total_turns = 0u32;
    let mut top_issues = Vec::new();
    let mut top_suggestions = Vec::new();
    let mut run_summaries = Vec::new();

    for run in &runs {
        total_turns += run.turn_count;
        match run.status {
            RunStatus::Completed => runs_completed += 1,
            RunStatus::Failed => runs_failed += 1,
            RunStatus::Running => runs_running += 1,
            RunStatus::Stale => {}
        }
        run_summaries.push(DailyRunSummary {
            run_id: run.run_id.clone(),
            goal: run.goal.clone(),
            status: run.status.as_str().to_string(),
            turn_count: run.turn_count,
        });
        if let Ok(Some(report)) = store.get_report(&run.run_id) {
            for issue in report.issues.iter().take(2) {
                if !top_issues.contains(&issue.message) {
                    top_issues.push(issue.message.clone());
                }
            }
            for sug in report.suggestions.iter().take(2) {
                if !top_suggestions.contains(&sug.message) {
                    top_suggestions.push(sug.message.clone());
                }
            }
        }
    }

    top_issues.truncate(5);
    top_suggestions.truncate(5);

    let summary = format!(
        "{} runs on {} — {} completed, {} in progress, {} failed, {} LLM turns total.",
        runs.len(),
        date,
        runs_completed,
        runs_running,
        runs_failed,
        total_turns
    );

    Ok(Some(DailyReport {
        date: date.to_string(),
        agent_id: agent_id.to_string(),
        display_name: agent.display_name.clone(),
        summary,
        runs_completed,
        runs_failed,
        runs_running,
        total_turns,
        top_issues,
        top_suggestions,
        run_summaries,
        generated_at: Utc::now(),
    }))
}

pub fn daily_report_markdown(report: &DailyReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("# AgentMirror Daily — {}\n\n", report.date));
    out.push_str(&format!("**{}** — {}\n\n", report.display_name, report.summary));
    out.push_str(&format!(
        "- Completed: {}\n- Running: {}\n- Failed: {}\n- Turns: {}\n\n",
        report.runs_completed, report.runs_running, report.runs_failed, report.total_turns
    ));
    if !report.run_summaries.is_empty() {
        out.push_str("## Runs\n");
        for r in &report.run_summaries {
            out.push_str(&format!(
                "- {} · {} · {} turns\n",
                r.goal, r.status, r.turn_count
            ));
        }
        out.push('\n');
    }
    if !report.top_issues.is_empty() {
        out.push_str("## Issues\n");
        for i in &report.top_issues {
            out.push_str(&format!("- {i}\n"));
        }
        out.push('\n');
    }
    if !report.top_suggestions.is_empty() {
        out.push_str("## Suggestions\n");
        for s in &report.top_suggestions {
            out.push_str(&format!("- {s}\n"));
        }
    }
    out
}

pub fn persist_graph(store: &InsightStore, run_id: &str) -> anyhow::Result<String> {
    let events = store.list_events(run_id)?;
    let graph = build_graph(run_id, &events);
    let json = serde_json::to_string_pretty(&graph)?;
    store.save_graph_json(run_id, &json)
}

pub fn finalize_run_if_idle(run: &mut RunRecord, last_activity: chrono::DateTime<Utc>) {
    let idle = Utc::now().signed_duration_since(last_activity);
    if run.status == RunStatus::Running && idle.num_minutes() > 30 {
        run.status = RunStatus::Completed;
        run.ended_at = Some(last_activity);
    }
}

pub fn outcome_from_status(status: RunStatus, outcome: RunOutcome) -> RunStatus {
    match outcome {
        RunOutcome::Failed => RunStatus::Failed,
        RunOutcome::Success if status == RunStatus::Running => RunStatus::Completed,
        _ => status,
    }
}
