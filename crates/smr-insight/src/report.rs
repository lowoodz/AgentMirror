use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;

use crate::critic::{evaluate, CriticInput};
use crate::graph::{build_graph, execution_summary};
use crate::infer::{generate_daily_report_llm, generate_reflection_report_llm, DailyRunReflectionInput};
use crate::locale::ReportLanguage;
use crate::llm::LlmClient;
use crate::models::{
    CognitiveEvent, DailyAgentSection, DailyIssueItem, DailyReport, DailyRunSummary,
    DailyTaskProgress, CriticsScore, DAILY_REPORT_ALL_AGENTS, Issue, ReflectionReport,
    RunOutcome, RunRecord, RunStatus,
};
use crate::safety::{scan_action_events, SafetyScanner};
use crate::store::InsightStore;

/// No new traffic for this long → treat run as done and generate LLM reflection report.
pub const RUN_REPORT_IDLE_MINUTES: i64 = 10;
/// While a run is still active, enqueue LLM reflection every N LLM turns.
pub const LLM_REFLECTION_TURN_INTERVAL: u32 = 10;
const DAILY_LIST_MAX: usize = 6;

fn run_duration_minutes(run: &RunRecord) -> Option<u32> {
    let end = run.ended_at.unwrap_or_else(Utc::now);
    let mins = end.signed_duration_since(run.started_at).num_minutes();
    if mins >= 0 {
        Some(mins.min(i64::from(u32::MAX)) as u32)
    } else {
        None
    }
}

pub fn run_short_id(run_id: &str) -> String {
    let s = run_id.trim();
    if s.len() <= 8 {
        s.to_string()
    } else {
        s[s.len().saturating_sub(6)..].to_string()
    }
}

fn lowest_critic_dimension(critics: &CriticsScore) -> (String, u8) {
    let dims = [
        ("Alignment", critics.alignment),
        ("Necessity", critics.necessity),
        ("Completeness", critics.completeness),
        ("Efficiency", critics.efficiency),
        ("Safety", critics.safety),
    ];
    dims.into_iter()
        .min_by_key(|(_, score)| *score)
        .map(|(name, score)| (name.to_string(), score))
        .unwrap_or_else(|| ("Completeness".to_string(), 70))
}

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
        language.empty_execution_summary().to_string()
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
            "LLM reflection report unavailable — keeping last LLM snapshot if any"
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
                    language,
                );
                return Ok(prior);
            }
        }
        anyhow::bail!(
            "LLM reflection report not available yet for run {}",
            run.run_id
        );
    }

    build_rule_reflection_report(run, &events, &execution_summary, &safety_findings, language)
}

fn llm_report_covers_state(
    prior: &ReflectionReport,
    run: &RunRecord,
    event_count: usize,
) -> bool {
    prior.llm_enhanced
        && prior.llm_run_status.as_deref() == Some(run.status.as_str())
        && prior.llm_turn_count >= run.turn_count
        && prior.llm_event_count >= event_count as u32
}

fn is_terminal_llm_trigger(run: &RunRecord, last_activity: DateTime<Utc>) -> bool {
    matches!(
        run.status,
        RunStatus::Completed | RunStatus::Failed | RunStatus::Stale
    ) || (run.status == RunStatus::Running && is_idle_for_report(last_activity))
}

fn is_periodic_turn_trigger(run: &RunRecord) -> bool {
    run.status == RunStatus::Running
        && run.turn_count >= LLM_REFLECTION_TURN_INTERVAL
        && run.turn_count.is_multiple_of(LLM_REFLECTION_TURN_INTERVAL)
}

/// LLM reports on run end / idle, and every [`LLM_REFLECTION_TURN_INTERVAL`] turns while running.
pub fn should_generate_llm_report(
    run: &RunRecord,
    event_count: usize,
    prior: Option<&ReflectionReport>,
    last_activity: DateTime<Utc>,
) -> bool {
    if event_count < 2 {
        return false;
    }

    if is_terminal_llm_trigger(run, last_activity) {
        if let Some(p) = prior {
            if llm_report_covers_state(p, run, event_count) {
                return false;
            }
        }
        return true;
    }

    if is_periodic_turn_trigger(run) {
        if let Some(p) = prior {
            if p.llm_enhanced && p.llm_turn_count >= run.turn_count {
                return false;
            }
        }
        return true;
    }

    false
}

/// Whether a stored report should be shown in the UI/API.
pub fn report_is_displayable(prior: &ReflectionReport, llm_critic: bool) -> bool {
    prior.llm_enhanced || !llm_critic
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReflectionReportAvailability {
    Ready,
    Generating,
    NotScheduled,
    InsufficientData,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReflectionReportStatus {
    pub availability: ReflectionReportAvailability,
    pub next_llm_turn: Option<u32>,
    pub turn_count: u32,
    pub event_count: usize,
}

fn next_llm_turn_milestone(run: &RunRecord) -> Option<u32> {
    let interval = LLM_REFLECTION_TURN_INTERVAL;
    let tc = run.turn_count;
    if tc < interval {
        Some(interval)
    } else {
        Some(((tc / interval) + 1) * interval)
    }
}

/// UI/API hint when no LLM reflection report is available yet.
pub fn reflection_report_status(
    run: &RunRecord,
    event_count: usize,
    prior: Option<&ReflectionReport>,
    last_activity: DateTime<Utc>,
    llm_critic: bool,
) -> ReflectionReportStatus {
    let turn_count = run.turn_count;
    if let Some(p) = prior {
        if report_is_displayable(p, llm_critic) {
            return ReflectionReportStatus {
                availability: ReflectionReportAvailability::Ready,
                next_llm_turn: None,
                turn_count,
                event_count,
            };
        }
    }

    if event_count < 2 {
        return ReflectionReportStatus {
            availability: ReflectionReportAvailability::InsufficientData,
            next_llm_turn: None,
            turn_count,
            event_count,
        };
    }

    if !llm_critic {
        return ReflectionReportStatus {
            availability: ReflectionReportAvailability::Generating,
            next_llm_turn: None,
            turn_count,
            event_count,
        };
    }

    if should_generate_llm_report(run, event_count, prior, last_activity) {
        return ReflectionReportStatus {
            availability: ReflectionReportAvailability::Generating,
            next_llm_turn: None,
            turn_count,
            event_count,
        };
    }

    ReflectionReportStatus {
        availability: ReflectionReportAvailability::NotScheduled,
        next_llm_turn: next_llm_turn_milestone(run),
        turn_count,
        event_count,
    }
}

pub fn refresh_running_llm_report(
    report: &mut ReflectionReport,
    run: &RunRecord,
    execution_summary: &str,
    events: &[crate::models::CognitiveEvent],
    safety_findings: &[String],
    language: ReportLanguage,
) {
    report.execution_summary = execution_summary.to_string();
    merge_safety_into_report(report, safety_findings);
    let (_, _, _, _, outcome) = evaluate(
        CriticInput {
            events,
            turn_count: run.turn_count,
            goal: &run.goal,
            safety_findings,
        },
        language,
    );
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

pub fn build_rule_reflection_report(
    run: &RunRecord,
    events: &[crate::models::CognitiveEvent],
    execution_summary: &str,
    safety_findings: &[String],
    language: ReportLanguage,
) -> anyhow::Result<ReflectionReport> {
    let (critics, critic_analyses, issues, suggestions, outcome) = evaluate(
        CriticInput {
            events,
            turn_count: run.turn_count,
            goal: &run.goal,
            safety_findings,
        },
        language,
    );

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
        llm_turn_count: 0,
        llm_run_status: None,
    })
}

/// Generate and persist an LLM reflection report for a run (invoked from the critic worker).
pub fn generate_llm_reflection_for_run(
    store: &InsightStore,
    run_id: &str,
    safety: Option<&dyn SafetyScanner>,
    llm: Option<&dyn LlmClient>,
    llm_critic: bool,
    language: ReportLanguage,
) -> anyhow::Result<bool> {
    if !llm_critic {
        return Ok(false);
    }
    let run = store
        .get_run(run_id)?
        .ok_or_else(|| anyhow::anyhow!("run not found: {run_id}"))?;
    let report = build_reflection_report(store, &run, safety, llm, llm_critic, language)?;
    let enhanced = report.llm_enhanced;
    let mut report = report;
    if enhanced {
        report.llm_turn_count = run.turn_count;
        report.llm_run_status = Some(run.status.as_str().to_string());
    }
    store.save_report(&report)?;
    Ok(enhanced)
}

/// Mark still-running runs completed and enqueue LLM report generation (post traffic replay).
pub fn finalize_runs_for_llm_reports(
    store: &InsightStore,
    llm_critic: bool,
) -> anyhow::Result<Vec<String>> {
    if !llm_critic {
        return Ok(Vec::new());
    }
    let mut run_ids = Vec::new();
    for mut run in store.list_runs(None, 10_000)? {
        if run.status != RunStatus::Running {
            continue;
        }
        run.status = RunStatus::Completed;
        if run.ended_at.is_none() {
            run.ended_at = Some(Utc::now());
        }
        store.update_run(&run)?;
        run_ids.push(run.run_id);
    }
    Ok(run_ids)
}

struct DailyReportInputs {
    runs_completed: u32,
    runs_failed: u32,
    runs_running: u32,
    total_turns: u32,
    active_agents: u32,
    run_summaries: Vec<DailyRunSummary>,
    run_inputs: Vec<DailyRunReflectionInput>,
    agent_sections: Vec<DailyAgentSection>,
}

fn collect_daily_report_inputs(
    store: &InsightStore,
    runs: &[RunRecord],
    agent_names: &std::collections::HashMap<String, String>,
) -> DailyReportInputs {
    let mut runs_completed = 0u32;
    let mut runs_failed = 0u32;
    let mut runs_running = 0u32;
    let mut total_turns = 0u32;
    let mut run_summaries = Vec::new();
    let mut run_inputs = Vec::new();
    let mut per_agent_runs: std::collections::HashMap<String, u32> =
        std::collections::HashMap::new();

    for run in runs {
        total_turns += run.turn_count;
        *per_agent_runs.entry(run.agent_id.clone()).or_insert(0) += 1;
        match run.status {
            RunStatus::Completed => runs_completed += 1,
            RunStatus::Failed => runs_failed += 1,
            RunStatus::Running => runs_running += 1,
            RunStatus::Stale => {}
        }
        let duration_minutes = run_duration_minutes(run);
        let status = run.status.as_str().to_string();
        run_summaries.push(DailyRunSummary {
            run_id: run.run_id.clone(),
            goal: run.goal.clone(),
            status: status.clone(),
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
                issue_msgs.push(issue.message.clone());
            }
            for sug in report.suggestions.iter().take(3) {
                suggestion_msgs.push(sug.message.clone());
            }
        }
        run_inputs.push(DailyRunReflectionInput {
            run_id: run.run_id.clone(),
            agent_id: run.agent_id.clone(),
            display_name,
            goal: run.goal.clone(),
            status,
            turn_count: run.turn_count,
            duration_minutes,
            reflection_summary,
            issues: issue_msgs,
            suggestions: suggestion_msgs,
        });
    }

    let active_agents = per_agent_runs.len() as u32;
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

    DailyReportInputs {
        runs_completed,
        runs_failed,
        runs_running,
        total_turns,
        active_agents,
        run_summaries,
        run_inputs,
        agent_sections,
    }
}

fn build_rule_daily_content(
    store: &InsightStore,
    runs: &[RunRecord],
    language: ReportLanguage,
) -> (Vec<DailyTaskProgress>, Vec<DailyIssueItem>, Vec<String>, Vec<String>) {
    let mut top_issues = Vec::new();
    let mut top_suggestions = Vec::new();
    let mut daily_issues = Vec::new();
    let mut task_progress = Vec::new();

    for run in runs {
        let duration_minutes = run_duration_minutes(run);
        let status = run.status.as_str().to_string();
        task_progress.push(DailyTaskProgress {
            run_id: run.run_id.clone(),
            goal: run.goal.clone(),
            status,
            turn_count: run.turn_count,
            duration_minutes,
        });

        if let Ok(Some(report)) = store.get_report(&run.run_id) {
            if let Some(issue) = report.issues.first() {
                let (dimension, score) = lowest_critic_dimension(&report.critics);
                let line = language.daily_issue_run_line(
                    &run_short_id(&run.run_id),
                    &issue.message,
                );
                if !top_issues.contains(&line) {
                    top_issues.push(line.clone());
                }
                daily_issues.push(DailyIssueItem {
                    run_id: run.run_id.clone(),
                    message: issue.message.clone(),
                    dimension: Some(dimension),
                    score: Some(score),
                });
            }
            for sug in report.suggestions.iter().take(2) {
                if !top_suggestions.contains(&sug.message) {
                    top_suggestions.push(sug.message.clone());
                }
            }
        }
    }

    top_issues.truncate(DAILY_LIST_MAX);
    top_suggestions.truncate(DAILY_LIST_MAX);
    daily_issues.truncate(DAILY_LIST_MAX);
    (task_progress, daily_issues, top_issues, top_suggestions)
}

/// Stable fingerprint of runs on a calendar day — used to detect stale daily reports.
/// Includes reflection report timestamps so a daily digest regenerates when underlying
/// reflection reports change.
pub fn daily_runs_fingerprint(store: &InsightStore, runs: &[RunRecord]) -> String {
    let mut parts: Vec<String> = runs
        .iter()
        .map(|r| {
            let report_ts = store
                .get_report(&r.run_id)
                .ok()
                .flatten()
                .map(|rep| rep.generated_at.timestamp())
                .unwrap_or(0);
            format!(
                "{}:{}:{}:{}",
                r.run_id,
                r.turn_count,
                r.status.as_str(),
                report_ts
            )
        })
        .collect();
    parts.sort();
    parts.join("|")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DailyReportAvailability {
    NoRuns,
    UpToDate,
    NeedsGeneration,
}

#[derive(Debug, Clone, Serialize)]
pub struct DailyReportStatus {
    pub availability: DailyReportAvailability,
    pub run_count: usize,
    pub has_llm_report: bool,
}

pub fn daily_report_status(
    store: &InsightStore,
    date: NaiveDate,
    llm_daily: bool,
) -> anyhow::Result<DailyReportStatus> {
    let runs = store.runs_on_date(date)?;
    if runs.is_empty() {
        return Ok(DailyReportStatus {
            availability: DailyReportAvailability::NoRuns,
            run_count: 0,
            has_llm_report: false,
        });
    }
    let fp = daily_runs_fingerprint(store, &runs);
    let reports = store.get_daily_report(&date.to_string(), None)?;
    if let Some(report) = reports.first() {
        let displayable = !llm_daily || report.llm_enhanced;
        if displayable && report.source_fingerprint.as_deref() == Some(fp.as_str()) {
            return Ok(DailyReportStatus {
                availability: DailyReportAvailability::UpToDate,
                run_count: runs.len(),
                has_llm_report: report.llm_enhanced,
            });
        }
    }
    Ok(DailyReportStatus {
        availability: DailyReportAvailability::NeedsGeneration,
        run_count: runs.len(),
        has_llm_report: reports.first().is_some_and(|r| r.llm_enhanced),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DailyGenerateOutcome {
    Generated,
    Unchanged,
    NoRuns,
    Failed,
}

/// Generate a daily report when needed, or return the existing one if still current.
pub fn try_generate_daily_report(
    store: &InsightStore,
    date: NaiveDate,
    llm: Option<&dyn LlmClient>,
    llm_daily: bool,
    language: ReportLanguage,
) -> anyhow::Result<(DailyGenerateOutcome, Option<DailyReport>)> {
    let status = daily_report_status(store, date, llm_daily)?;
    if status.availability == DailyReportAvailability::NoRuns {
        return Ok((DailyGenerateOutcome::NoRuns, None));
    }
    if status.availability == DailyReportAvailability::UpToDate {
        let reports = store.get_daily_report(&date.to_string(), None)?;
        return Ok((DailyGenerateOutcome::Unchanged, reports.into_iter().next()));
    }

    let runs = store.runs_on_date(date)?;
    let fingerprint = daily_runs_fingerprint(store, &runs);
    let Some(mut report) =
        generate_all_agents_daily_report(store, date, llm, llm_daily, language, &fingerprint)?
    else {
        return Ok((DailyGenerateOutcome::Failed, None));
    };
    report.source_fingerprint = Some(fingerprint);
    Ok((DailyGenerateOutcome::Generated, Some(report)))
}

fn assemble_daily_report(
    date: NaiveDate,
    language: ReportLanguage,
    inputs: &DailyReportInputs,
    task_progress: Vec<DailyTaskProgress>,
    daily_issues: Vec<DailyIssueItem>,
    top_issues: Vec<String>,
    top_suggestions: Vec<String>,
    llm_enhanced: bool,
    source_fingerprint: Option<String>,
) -> DailyReport {
    let summary = language.daily_baseline_summary(
        inputs.run_summaries.len(),
        date,
        inputs.runs_completed,
        inputs.runs_running,
        inputs.runs_failed,
        inputs.total_turns,
    );

    DailyReport {
        date: date.to_string(),
        agent_id: DAILY_REPORT_ALL_AGENTS.to_string(),
        display_name: language.daily_all_agents_label().to_string(),
        summary,
        active_agents: inputs.active_agents,
        runs_completed: inputs.runs_completed,
        runs_failed: inputs.runs_failed,
        runs_running: inputs.runs_running,
        total_turns: inputs.total_turns,
        task_progress,
        daily_issues,
        top_issues,
        top_suggestions,
        run_summaries: inputs.run_summaries.clone(),
        generated_at: Utc::now(),
        tasks_overview: None,
        progress_narrative: None,
        llm_enhanced,
        source_fingerprint,
        report_language: language.as_str().to_string(),
        agent_sections: inputs.agent_sections.clone(),
    }
}

pub fn generate_all_agents_daily_report(
    store: &InsightStore,
    date: NaiveDate,
    llm: Option<&dyn LlmClient>,
    llm_daily: bool,
    language: ReportLanguage,
    source_fingerprint: &str,
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

    let inputs = collect_daily_report_inputs(store, &runs, &agent_names);

    if llm_daily {
        let Some(client) = llm else {
            tracing::warn!(
                date = %date,
                "AgentMirror daily LLM required but no critic client configured"
            );
            return Ok(None);
        };
        let shell = assemble_daily_report(
            date,
            language,
            &inputs,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            false,
            Some(source_fingerprint.to_string()),
        );
        let Some(mut report) =
            generate_daily_report_llm(client, date, &shell, &inputs.run_inputs, language)
        else {
            tracing::warn!(
                date = %date,
                "AgentMirror daily LLM failed — no report generated"
            );
            return Ok(None);
        };
        report.llm_enhanced = true;
        report.source_fingerprint = Some(source_fingerprint.to_string());
        return Ok(Some(report));
    }

    let (task_progress, daily_issues, top_issues, top_suggestions) =
        build_rule_daily_content(store, &runs, language);
    Ok(Some(assemble_daily_report(
        date,
        language,
        &inputs,
        task_progress,
        daily_issues,
        top_issues,
        top_suggestions,
        false,
        Some(source_fingerprint.to_string()),
    )))
}

/// Per-agent daily reports are deprecated; kept as alias for tests/scripts.
pub fn generate_daily_report(
    store: &InsightStore,
    _agent_id: &str,
    date: NaiveDate,
) -> anyhow::Result<Option<DailyReport>> {
    let runs = store.runs_on_date(date)?;
    let fp = daily_runs_fingerprint(store, &runs);
    generate_all_agents_daily_report(store, date, None, false, ReportLanguage::En, &fp)
}

pub fn daily_report_markdown(report: &DailyReport) -> String {
    let zh = report.language() == ReportLanguage::Zh;
    let badge = if report.llm_enhanced { "LLM" } else { "规则基线" };
    let badge_en = if report.llm_enhanced {
        "LLM"
    } else {
        "Rule baseline"
    };
    let generated = report
        .generated_at
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();

    let mut out = String::new();
    if zh {
        out.push_str(&format!(
            "# {} · 日报             生成于 {} （{}）\n\n",
            report.display_name, generated, badge
        ));
        out.push_str("## 总览\n");
        out.push_str(&format!("- 活跃 Agent: {}\n", report.active_agents));
        out.push_str(&format!(
            "- 完成任务: {} / 进行中: {} / 失败: {}\n",
            report.runs_completed, report.runs_running, report.runs_failed
        ));
        out.push_str(&format!("- 总 LLM 轮次: {}\n\n", report.total_turns));
    } else {
        out.push_str(&format!(
            "# {} · Daily             Generated {} ({}) \n\n",
            report.display_name, generated, badge_en
        ));
        out.push_str("## Overview\n");
        out.push_str(&format!("- Active agents: {}\n", report.active_agents));
        out.push_str(&format!(
            "- Completed: {} / In progress: {} / Failed: {}\n",
            report.runs_completed, report.runs_running, report.runs_failed
        ));
        out.push_str(&format!("- Total LLM turns: {}\n\n", report.total_turns));
    }

    let tasks = if !report.task_progress.is_empty() {
        report.task_progress.clone()
    } else {
        report
            .run_summaries
            .iter()
            .map(|r| DailyTaskProgress {
                run_id: r.run_id.clone(),
                goal: r.goal.clone(),
                status: r.status.clone(),
                turn_count: r.turn_count,
                duration_minutes: None,
            })
            .collect()
    };

    if !tasks.is_empty() {
        if zh {
            out.push_str("### 任务及进展\n");
        } else {
            out.push_str("### Tasks & progress\n");
        }
        for (idx, task) in tasks.iter().enumerate() {
            let icon = task_status_icon(&task.status);
            let detail = format_task_progress_line(task, zh);
            out.push_str(&format!("{}. {} {}\n", idx + 1, icon, detail));
        }
        out.push('\n');
    }

    let issues = if !report.daily_issues.is_empty() {
        &report.daily_issues[..]
    } else {
        &[]
    };
    if !issues.is_empty() || !report.top_issues.is_empty() {
        if zh {
            out.push_str("### 问题/风险\n");
        } else {
            out.push_str("### Issues & risks\n");
        }
        if !issues.is_empty() {
            for issue in issues {
                out.push_str(&format!(
                    "- Run #{}: {}{}\n",
                    run_short_id(&issue.run_id),
                    issue.message,
                    format_issue_score_suffix(issue, zh)
                ));
            }
        } else {
            for i in &report.top_issues {
                out.push_str(&format!("- {i}\n"));
            }
        }
        out.push('\n');
    }

    if !report.top_suggestions.is_empty() {
        if zh {
            out.push_str("## 改进建议\n");
        } else {
            out.push_str("## Recommendations\n");
        }
        for s in &report.top_suggestions {
            out.push_str(&format!("- {s}\n"));
        }
    }
    out
}

pub fn task_status_icon(status: &str) -> &'static str {
    let s = status.to_ascii_lowercase();
    if s.contains("complete") || s.contains("done") || s.contains("success") {
        "✓"
    } else if s.contains("fail") || s.contains("error") {
        "✗"
    } else {
        "→"
    }
}

pub fn format_task_progress_line(task: &DailyTaskProgress, zh: bool) -> String {
    let s = task.status.to_ascii_lowercase();
    if s.contains("run") && !s.contains("complete") {
        if zh {
            return format!("{} — 进行中", task.goal);
        }
        return format!("{} — in progress", task.goal);
    }
    if let Some(mins) = task.duration_minutes {
        if zh {
            format!("{} — {} 步，{} 分钟", task.goal, task.turn_count, mins)
        } else {
            format!(
                "{} — {} steps, {} min",
                task.goal, task.turn_count, mins
            )
        }
    } else if zh {
        format!("{} — {} 步", task.goal, task.turn_count)
    } else {
        format!("{} — {} steps", task.goal, task.turn_count)
    }
}

pub fn format_issue_score_suffix(issue: &DailyIssueItem, zh: bool) -> String {
    match (&issue.dimension, issue.score) {
        (Some(dim), Some(score)) if zh => format!("（{} {}分）", dim, score),
        (Some(dim), Some(score)) => format!(" ({dim} {score})"),
        _ => String::new(),
    }
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

/// Background sweep: finalize idle running runs and return run ids needing LLM reports.
pub fn sweep_idle_running_runs(
    store: &InsightStore,
    llm_critic: bool,
) -> anyhow::Result<Vec<String>> {
    if !llm_critic {
        return Ok(Vec::new());
    }
    let mut run_ids = Vec::new();
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
        run_ids.push(run.run_id);
    }
    Ok(run_ids)
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
    fn generates_llm_on_every_tenth_turn_while_running() {
        let run = RunRecord {
            run_id: "r1".into(),
            agent_id: "a1".into(),
            session_id: "s1".into(),
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
            status: RunStatus::Running,
            goal: "task".into(),
            turn_count: 10,
            messages_seen: 0,
            graph_path: None,
        };
        assert!(should_generate_llm_report(
            &run,
            10,
            None,
            Utc::now()
        ));
    }

    #[test]
    fn skips_duplicate_periodic_llm_on_same_turn() {
        let run = RunRecord {
            run_id: "r1".into(),
            agent_id: "a1".into(),
            session_id: "s1".into(),
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
            status: RunStatus::Running,
            goal: "task".into(),
            turn_count: 10,
            messages_seen: 0,
            graph_path: None,
        };
        let prior = ReflectionReport {
            run_id: "r1".into(),
            goal: "task".into(),
            original_goal: Some("task".into()),
            execution_summary: String::new(),
            outcome: RunOutcome::Partial,
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
            reflection_summary: Some("mid-run".into()),
            llm_enhanced: true,
            llm_event_count: 10,
            llm_turn_count: 10,
            llm_run_status: Some("running".into()),
        };
        assert!(!should_generate_llm_report(
            &run,
            10,
            Some(&prior),
            Utc::now()
        ));
    }

    #[test]
    fn terminal_llm_runs_again_after_periodic_snapshot() {
        let run = RunRecord {
            run_id: "r1".into(),
            agent_id: "a1".into(),
            session_id: "s1".into(),
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
            status: RunStatus::Completed,
            goal: "task".into(),
            turn_count: 12,
            messages_seen: 0,
            graph_path: None,
        };
        let prior = ReflectionReport {
            run_id: "r1".into(),
            goal: "task".into(),
            original_goal: Some("task".into()),
            execution_summary: String::new(),
            outcome: RunOutcome::Partial,
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
            reflection_summary: Some("mid-run".into()),
            llm_enhanced: true,
            llm_event_count: 10,
            llm_turn_count: 10,
            llm_run_status: Some("running".into()),
        };
        assert!(should_generate_llm_report(
            &run,
            12,
            Some(&prior),
            Utc::now()
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
            llm_turn_count: 5,
            llm_run_status: Some("completed".into()),
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

    fn sample_run(turn_count: u32, status: RunStatus) -> RunRecord {
        RunRecord {
            run_id: "r1".into(),
            agent_id: "a1".into(),
            session_id: "s1".into(),
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
            status,
            goal: "task".into(),
            turn_count,
            messages_seen: 0,
            graph_path: None,
        }
    }

    #[test]
    fn daily_status_needs_regeneration_when_reflection_updates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store =
            InsightStore::open(dir.path(), dir.path().join("graphs")).expect("store");
        let date = chrono::Local::now().date_naive();
        store
            .upsert_agent(&crate::models::AgentRecord {
                agent_id: "a1".into(),
                display_name: "Agent".into(),
                agent_type: "agent".into(),
                system_hash: String::new(),
                tools_json: "[]".into(),
                first_seen: Utc::now(),
                last_seen: Utc::now(),
            })
            .expect("agent");
        let run = RunRecord {
            run_id: "r1".into(),
            agent_id: "a1".into(),
            session_id: "s1".into(),
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
            status: RunStatus::Completed,
            goal: "task".into(),
            turn_count: 3,
            messages_seen: 0,
            graph_path: None,
        };
        store.insert_run(&run).expect("run");
        let fp_before = daily_runs_fingerprint(&store, std::slice::from_ref(&run));
        let report = DailyReport {
            date: date.to_string(),
            agent_id: DAILY_REPORT_ALL_AGENTS.to_string(),
            display_name: "All".into(),
            summary: String::new(),
            active_agents: 1,
            runs_completed: 1,
            runs_failed: 0,
            runs_running: 0,
            total_turns: 3,
            task_progress: vec![],
            daily_issues: vec![],
            top_issues: vec![],
            top_suggestions: vec![],
            run_summaries: vec![],
            generated_at: Utc::now(),
            tasks_overview: None,
            progress_narrative: None,
            llm_enhanced: true,
            source_fingerprint: Some(fp_before),
            report_language: "en".into(),
            agent_sections: vec![],
        };
        store.save_daily_report(&report).expect("save");
        store
            .save_report(&ReflectionReport {
                run_id: "r1".into(),
                goal: "task".into(),
                original_goal: Some("task".into()),
                execution_summary: "done".into(),
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
                reflection_summary: Some("summary".into()),
                llm_enhanced: true,
                llm_event_count: 3,
                llm_turn_count: 3,
                llm_run_status: Some("completed".into()),
            })
            .expect("reflection");
        let status = daily_report_status(&store, date, true).expect("status");
        assert_eq!(
            status.availability,
            DailyReportAvailability::NeedsGeneration
        );
    }

    #[test]
    fn daily_status_up_to_date_when_fingerprint_matches() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store =
            InsightStore::open(dir.path(), dir.path().join("graphs")).expect("store");
        let date = chrono::Local::now().date_naive();
        store
            .upsert_agent(&crate::models::AgentRecord {
                agent_id: "a1".into(),
                display_name: "Agent".into(),
                agent_type: "agent".into(),
                system_hash: String::new(),
                tools_json: "[]".into(),
                first_seen: Utc::now(),
                last_seen: Utc::now(),
            })
            .expect("agent");
        let run = RunRecord {
            run_id: "r1".into(),
            agent_id: "a1".into(),
            session_id: "s1".into(),
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
            status: RunStatus::Completed,
            goal: "task".into(),
            turn_count: 3,
            messages_seen: 0,
            graph_path: None,
        };
        store.insert_run(&run).expect("run");
        let fp = daily_runs_fingerprint(&store, std::slice::from_ref(&run));
        let report = DailyReport {
            date: date.to_string(),
            agent_id: DAILY_REPORT_ALL_AGENTS.to_string(),
            display_name: "All".into(),
            summary: String::new(),
            active_agents: 1,
            runs_completed: 1,
            runs_failed: 0,
            runs_running: 0,
            total_turns: 3,
            task_progress: vec![],
            daily_issues: vec![],
            top_issues: vec![],
            top_suggestions: vec![],
            run_summaries: vec![],
            generated_at: Utc::now(),
            tasks_overview: None,
            progress_narrative: None,
            llm_enhanced: true,
            source_fingerprint: Some(fp),
            report_language: "en".into(),
            agent_sections: vec![],
        };
        store.save_daily_report(&report).expect("save");
        let status = daily_report_status(&store, date, true).expect("status");
        assert_eq!(status.availability, DailyReportAvailability::UpToDate);
    }

    #[test]
    fn llm_daily_requires_client() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store =
            InsightStore::open(dir.path(), dir.path().join("graphs")).expect("store");
        let date = chrono::Local::now().date_naive();
        store
            .upsert_agent(&crate::models::AgentRecord {
                agent_id: "a1".into(),
                display_name: "Agent".into(),
                agent_type: "agent".into(),
                system_hash: String::new(),
                tools_json: "[]".into(),
                first_seen: Utc::now(),
                last_seen: Utc::now(),
            })
            .expect("agent");
        let run = RunRecord {
            run_id: "r1".into(),
            agent_id: "a1".into(),
            session_id: "s1".into(),
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
            status: RunStatus::Completed,
            goal: "task".into(),
            turn_count: 3,
            messages_seen: 0,
            graph_path: None,
        };
        store.insert_run(&run).expect("run");
        let (outcome, report) =
            try_generate_daily_report(&store, date, None, true, ReportLanguage::En).expect("ok");
        assert_eq!(outcome, DailyGenerateOutcome::Failed);
        assert!(report.is_none());
    }

    #[test]
    fn reflection_status_not_scheduled_before_milestone() {
        let run = sample_run(5, RunStatus::Running);
        let status = reflection_report_status(&run, 10, None, Utc::now(), true);
        assert_eq!(
            status.availability,
            ReflectionReportAvailability::NotScheduled
        );
        assert_eq!(status.next_llm_turn, Some(10));
    }

    #[test]
    fn reflection_status_generating_at_milestone() {
        let run = sample_run(10, RunStatus::Running);
        let status = reflection_report_status(&run, 10, None, Utc::now(), true);
        assert_eq!(
            status.availability,
            ReflectionReportAvailability::Generating
        );
    }

    #[test]
    fn reflection_status_ready_when_llm_enhanced() {
        let run = sample_run(10, RunStatus::Running);
        let prior = ReflectionReport {
            run_id: "r1".into(),
            goal: "task".into(),
            original_goal: None,
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
            reflection_summary: None,
            llm_enhanced: true,
            llm_event_count: 10,
            llm_turn_count: 10,
            llm_run_status: Some("running".into()),
        };
        let status = reflection_report_status(&run, 10, Some(&prior), Utc::now(), true);
        assert_eq!(status.availability, ReflectionReportAvailability::Ready);
    }
}