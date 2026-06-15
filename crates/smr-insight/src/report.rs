use chrono::{DateTime, NaiveDate, Utc};

use crate::critic::{evaluate, CriticInput};
use crate::graph::{build_graph, execution_summary};
use crate::infer::{generate_daily_report_llm, generate_reflection_report_llm, DailyRunReflectionInput};
use crate::llm::LlmClient;
use crate::locale::ReportLanguage;
use crate::models::{
    CognitiveEvent, DailyAgentSection, DailyReport, DailyRunSummary, DAILY_REPORT_ALL_AGENTS,
    Issue, ReflectionReport, RunOutcome, RunRecord, RunStatus,
};
use crate::safety::{scan_action_events, SafetyScanner};
use crate::store::InsightStore;

/// No new traffic for this long → treat run as done and generate LLM reflection report.
pub const RUN_REPORT_IDLE_MINUTES: i64 = 10;

pub fn last_activity_at(run: &RunRecord, events: &[CognitiveEvent]) -> DateTime<Utc> {
    events
        .iter()
        .map(|e| e.timestamp)
        .max()
        .or(run.ended_at)
        .unwrap_or(run.started_at)
}

pub fn is_idle_for_report(last_activity: DateTime<Utc>) -> bool {
    Utc::now()
        .signed_duration_since(last_activity)
        .num_minutes()
        >= RUN_REPORT_IDLE_MINUTES
}

pub fn build_reflection_report(
    store: &InsightStore,
    run: &RunRecord,
    safety: Option<&dyn SafetyScanner>,
    llm: Option<&dyn LlmClient>,
    llm_critic: bool,
    language: ReportLanguage,
) -> anyhow::Result<ReflectionReport> {
    let events = store.list_events(&run.run_id)?;
    let safety_findings = scan_action_events(&events, safety);

    let summary = execution_summary(&events);
    let execution_summary = if summary.is_empty() {
        "No actions recorded yet".to_string()
    } else {
        summary
    };

    let prior = store.get_report(&run.run_id).ok().flatten();
    let last_activity = last_activity_at(run, &events);

    if llm_critic
        && should_generate_llm_report(run, events.len(), prior.as_ref(), last_activity)
    {
        if let Some(report) = generate_reflection_report_llm(
            llm,
            run,
            &events,
            &execution_summary,
            &safety_findings,
            prior.as_ref(),
            language,
        ) {
            return Ok(report);
        }
        tracing::warn!(
            run_id = %run.run_id,
            "LLM reflection report unavailable — falling back to rule baseline"
        );
    }

    // While run is active, keep the last LLM report and refresh lightweight metadata.
    if llm_critic {
        if let Some(mut prior) = prior {
            if prior.llm_enhanced {
                refresh_running_llm_report(
                    &mut prior,
                    run,
                    &execution_summary,
                    &events,
                    &safety_findings,
                );
                return Ok(prior);
            }
        }
    }

    build_rule_reflection_report(run, &events, &execution_summary, &safety_findings)
}

/// LLM reports when the run ends, or when the last event is idle ≥ RUN_REPORT_IDLE_MINUTES.
fn should_generate_llm_report(
    run: &RunRecord,
    event_count: usize,
    prior: Option<&ReflectionReport>,
    last_activity: DateTime<Utc>,
) -> bool {
    if event_count < 2 {
        return false;
    }
    if prior
        .map(|p| p.llm_enhanced && p.llm_event_count >= event_count as u32)
        .unwrap_or(false)
    {
        return false;
    }
    match run.status {
        RunStatus::Completed | RunStatus::Failed | RunStatus::Stale => true,
        RunStatus::Running => is_idle_for_report(last_activity),
    }
}

fn refresh_running_llm_report(
    report: &mut ReflectionReport,
    run: &RunRecord,
    execution_summary: &str,
    events: &[crate::models::CognitiveEvent],
    safety_findings: &[String],
) {
    report.execution_summary = execution_summary.to_string();
    merge_safety_into_report(report, safety_findings);
    let (_, _, _, _, outcome) = evaluate(CriticInput {
        events,
        turn_count: run.turn_count,
        goal: &run.goal,
        safety_findings,
    });
    report.outcome = outcome;
}

pub fn merge_safety_into_report(report: &mut ReflectionReport, safety_findings: &[String]) {
    for finding in safety_findings {
        let issue = Issue {
            message: finding.clone(),
            severity: "high".to_string(),
        };
        if !report
            .issues
            .iter()
            .any(|i| i.message == issue.message)
        {
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

fn build_rule_reflection_report(
    run: &RunRecord,
    events: &[crate::models::CognitiveEvent],
    execution_summary: &str,
    safety_findings: &[String],
) -> anyhow::Result<ReflectionReport> {
    let (critics, critic_analyses, issues, suggestions, outcome) = evaluate(CriticInput {
        events,
        turn_count: run.turn_count,
        goal: &run.goal,
        safety_findings,
    });

    let risks = issues
        .iter()
        .filter(|i| i.severity == "high")
        .map(|i| i.message.clone())
        .collect();

    Ok(ReflectionReport {
        run_id: run.run_id.clone(),
        goal: run.goal.clone(),
        original_goal: None,
        execution_summary: execution_summary.to_string(),
        outcome,
        issues,
        risks,
        suggestions,
        critics,
        critic_analyses,
        generated_at: Utc::now(),
        dialectical: None,
        counterfactuals: Vec::new(),
        estimated_improvement: None,
        logical_analysis: None,
        reflection_summary: None,
        llm_enhanced: false,
        llm_event_count: 0,
    })
}

/// Mark still-running runs completed and regenerate LLM reports (post traffic replay).
pub fn finalize_runs_for_llm_reports(
    store: &InsightStore,
    safety: Option<&dyn SafetyScanner>,
    llm: Option<&dyn LlmClient>,
    llm_critic: bool,
    language: ReportLanguage,
) -> anyhow::Result<usize> {
    if !llm_critic {
        return Ok(0);
    }
    let mut updated = 0usize;
    for mut run in store.list_runs(None, 10_000)? {
        if run.status != RunStatus::Running {
            continue;
        }
        run.status = RunStatus::Completed;
        if run.ended_at.is_none() {
            run.ended_at = Some(Utc::now());
        }
        store.update_run(&run)?;
        let report = build_reflection_report(store, &run, safety, llm, llm_critic, language)?;
        store.save_report(&report)?;
        updated += 1;
    }
    Ok(updated)
}

pub fn generate_all_agents_daily_report(
    store: &InsightStore,
    date: NaiveDate,
    llm: Option<&dyn LlmClient>,
    llm_daily: bool,
    language: ReportLanguage,
) -> anyhow::Result<Option<DailyReport>> {
    let runs = store.runs_on_date(date)?;
    if runs.is_empty() {
        return Ok(None);
    }

    let agents = store.list_agents(500)?;
    let agent_names: std::collections::HashMap<String, String> = agents
        .into_iter()
        .map(|a| (a.agent_id.clone(), a.display_name))
        .collect();

    let mut runs_completed = 0u32;
    let mut runs_failed = 0u32;
    let mut runs_running = 0u32;
    let mut total_turns = 0u32;
    let mut top_issues = Vec::new();
    let mut top_suggestions = Vec::new();
    let mut run_summaries = Vec::new();
    let mut run_inputs = Vec::new();
    let mut per_agent_runs: std::collections::HashMap<String, u32> =
        std::collections::HashMap::new();

    for run in &runs {
        total_turns += run.turn_count;
        *per_agent_runs.entry(run.agent_id.clone()).or_insert(0) += 1;
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

        let display_name = agent_names
            .get(&run.agent_id)
            .cloned()
            .unwrap_or_else(|| run.agent_id.clone());

        let mut reflection_summary = None;
        let mut issue_msgs = Vec::new();
        let mut suggestion_msgs = Vec::new();
        if let Ok(Some(report)) = store.get_report(&run.run_id) {
            reflection_summary = report
                .reflection_summary
                .clone()
                .or_else(|| {
                    if !report.execution_summary.is_empty() {
                        Some(report.execution_summary.clone())
                    } else {
                        None
                    }
                });
            for issue in report.issues.iter().take(3) {
                if !top_issues.contains(&issue.message) {
                    top_issues.push(issue.message.clone());
                }
                issue_msgs.push(issue.message.clone());
            }
            for sug in report.suggestions.iter().take(3) {
                if !top_suggestions.contains(&sug.message) {
                    top_suggestions.push(sug.message.clone());
                }
                suggestion_msgs.push(sug.message.clone());
            }
        }
        run_inputs.push(DailyRunReflectionInput {
            agent_id: run.agent_id.clone(),
            display_name,
            goal: run.goal.clone(),
            status: run.status.as_str().to_string(),
            turn_count: run.turn_count,
            reflection_summary,
            issues: issue_msgs,
            suggestions: suggestion_msgs,
        });
    }

    top_issues.truncate(8);
    top_suggestions.truncate(8);

    let agent_sections: Vec<DailyAgentSection> = per_agent_runs
        .into_iter()
        .map(|(agent_id, run_count)| DailyAgentSection {
            display_name: agent_names
                .get(&agent_id)
                .cloned()
                .unwrap_or_else(|| agent_id.clone()),
            agent_id,
            summary: String::new(),
            run_count,
        })
        .collect();

    let summary = format!(
        "{} runs on {} — {} completed, {} in progress, {} failed, {} LLM turns total.",
        runs.len(),
        date,
        runs_completed,
        runs_running,
        runs_failed,
        total_turns
    );

    let mut report = DailyReport {
        date: date.to_string(),
        agent_id: DAILY_REPORT_ALL_AGENTS.to_string(),
        display_name: language.daily_all_agents_label().to_string(),
        summary,
        runs_completed,
        runs_failed,
        runs_running,
        total_turns,
        top_issues,
        top_suggestions,
        run_summaries,
        generated_at: Utc::now(),
        tasks_overview: None,
        progress_narrative: None,
        llm_enhanced: false,
        agent_sections,
    };

    if llm_daily {
        if let Some(client) = llm {
            if let Some(enhanced) =
                generate_daily_report_llm(client, date, &report, &run_inputs, language)
            {
                report = enhanced;
            } else {
                tracing::warn!(
                    date = %date,
                    "AgentMirror daily LLM unavailable — using rule baseline"
                );
            }
        }
    }

    Ok(Some(report))
}

/// Per-agent daily reports are deprecated; kept as alias for tests/scripts.
pub fn generate_daily_report(
    store: &InsightStore,
    _agent_id: &str,
    date: NaiveDate,
) -> anyhow::Result<Option<DailyReport>> {
    generate_all_agents_daily_report(store, date, None, false, ReportLanguage::En)
}

pub fn daily_report_markdown(report: &DailyReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("# AgentMirror Daily — {}\n\n", report.date));
    out.push_str(&format!("**{}** — {}\n\n", report.display_name, report.summary));
    out.push_str(&format!(
        "- Completed: {}\n- Running: {}\n- Failed: {}\n- Turns: {}\n\n",
        report.runs_completed, report.runs_running, report.runs_failed, report.total_turns
    ));
    if let Some(tasks) = &report.tasks_overview {
        out.push_str(&format!("## Tasks\n{tasks}\n\n"));
    }
    if let Some(progress) = &report.progress_narrative {
        out.push_str(&format!("## Progress\n{progress}\n\n"));
    }
    if !report.agent_sections.is_empty() {
        out.push_str("## Agents\n");
        for a in &report.agent_sections {
            out.push_str(&format!(
                "### {} ({} runs)\n{}\n\n",
                a.display_name, a.run_count, a.summary
            ));
        }
    }
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

pub fn finalize_run_if_idle(run: &mut RunRecord, last_activity: DateTime<Utc>) {
    let idle = Utc::now().signed_duration_since(last_activity);
    if run.status == RunStatus::Running && idle.num_minutes() >= RUN_REPORT_IDLE_MINUTES {
        run.status = RunStatus::Completed;
        run.ended_at = Some(last_activity);
    }
}

/// Background sweep: finalize idle running runs and generate LLM reports.
pub fn sweep_idle_running_runs(
    store: &InsightStore,
    safety: Option<&dyn SafetyScanner>,
    llm: Option<&dyn LlmClient>,
    llm_critic: bool,
    language: ReportLanguage,
) -> anyhow::Result<usize> {
    if !llm_critic {
        return Ok(0);
    }
    let mut updated = 0usize;
    for mut run in store.list_runs(None, 10_000)? {
        if run.status != RunStatus::Running {
            continue;
        }
        let events = store.list_events(&run.run_id)?;
        if events.len() < 2 {
            continue;
        }
        let last = last_activity_at(&run, &events);
        if !is_idle_for_report(last) {
            continue;
        }
        finalize_run_if_idle(&mut run, last);
        store.update_run(&run)?;
        let report = build_reflection_report(store, &run, safety, llm, llm_critic, language)?;
        store.save_report(&report)?;
        if report.llm_enhanced {
            updated += 1;
            tracing::info!(
                run_id = %run.run_id,
                idle_minutes = RUN_REPORT_IDLE_MINUTES,
                "AgentMirror idle run — LLM reflection report generated"
            );
        }
    }
    Ok(updated)
}

pub fn outcome_from_status(status: RunStatus, outcome: RunOutcome) -> RunStatus {
    match outcome {
        RunOutcome::Failed if status == RunStatus::Running => RunStatus::Failed,
        _ => status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::CriticsScore;

    #[test]
    fn defers_llm_while_running_and_recent() {
        let run = RunRecord {
            run_id: "r1".into(),
            agent_id: "a1".into(),
            session_id: "s1".into(),
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
            status: RunStatus::Running,
            goal: "task".into(),
            turn_count: 5,
            messages_seen: 0,
            graph_path: None,
        };
        assert!(!should_generate_llm_report(
            &run,
            10,
            None,
            Utc::now()
        ));
    }

    #[test]
    fn generates_llm_when_running_but_idle() {
        let run = RunRecord {
            run_id: "r1".into(),
            agent_id: "a1".into(),
            session_id: "s1".into(),
            started_at: Utc::now() - chrono::Duration::minutes(20),
            ended_at: Some(Utc::now() - chrono::Duration::minutes(15)),
            status: RunStatus::Running,
            goal: "task".into(),
            turn_count: 5,
            messages_seen: 0,
            graph_path: None,
        };
        let last = Utc::now() - chrono::Duration::minutes(11);
        assert!(should_generate_llm_report(&run, 10, None, last));
    }

    #[test]
    fn generates_llm_when_run_completed() {
        let run = RunRecord {
            run_id: "r1".into(),
            agent_id: "a1".into(),
            session_id: "s1".into(),
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
            status: RunStatus::Completed,
            goal: "task".into(),
            turn_count: 5,
            messages_seen: 0,
            graph_path: None,
        };
        assert!(should_generate_llm_report(
            &run,
            10,
            None,
            Utc::now() - chrono::Duration::minutes(1)
        ));
    }

    #[test]
    fn skips_llm_when_report_already_current() {
        let run = RunRecord {
            run_id: "r1".into(),
            agent_id: "a1".into(),
            session_id: "s1".into(),
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
            status: RunStatus::Completed,
            goal: "task".into(),
            turn_count: 5,
            messages_seen: 0,
            graph_path: None,
        };
        let prior = ReflectionReport {
            run_id: "r1".into(),
            goal: "task".into(),
            original_goal: Some("task".into()),
            execution_summary: String::new(),
            outcome: RunOutcome::Success,
            issues: vec![],
            risks: vec![],
            suggestions: vec![],
            critics: CriticsScore::default(),
            critic_analyses: Default::default(),
            generated_at: Utc::now(),
            dialectical: None,
            counterfactuals: vec![],
            estimated_improvement: None,
            logical_analysis: None,
            reflection_summary: Some("done".into()),
            llm_enhanced: true,
            llm_event_count: 10,
        };
        assert!(!should_generate_llm_report(
            &run,
            10,
            Some(&prior),
            Utc::now() - chrono::Duration::minutes(1)
        ));
    }

    #[test]
    fn is_idle_for_report_after_ten_minutes() {
        let last = Utc::now() - chrono::Duration::minutes(10);
        assert!(is_idle_for_report(last));
        let recent = Utc::now() - chrono::Duration::minutes(9);
        assert!(!is_idle_for_report(recent));
    }
}