use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::Value;

use crate::http_state::HttpState;
use crate::insight_risk::{load_audits_for_ids, risk_for_run, risk_for_runs};
use smr_insight::export::daily_reports_html;
use smr_insight::models::EventKind;
use smr_insight::pattern::{mine_patterns, pattern_matches_run};
use smr_insight::profile::build_profile;

#[derive(Deserialize)]
struct RunsQuery {
    agent_id: Option<String>,
    limit: Option<usize>,
    date: Option<String>,
}

#[derive(Deserialize)]
struct AgentsQuery {
    date: Option<String>,
    limit: Option<usize>,
}

fn parse_insight_date(date: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()
}

#[derive(Deserialize)]
struct DailyQuery {
    agent_id: Option<String>,
}

#[derive(Deserialize)]
struct GenerateDailyRequest {
    date: Option<String>,
}

#[derive(Deserialize)]
struct MergeRunsRequest {
    target_run_id: String,
    source_run_ids: Vec<String>,
}

#[derive(Deserialize)]
struct SplitRunRequest {
    after_seq: u32,
}

pub fn router() -> Router<HttpState> {
    Router::new()
        .route("/api/insight/status", get(api_insight_status))
        .route("/api/insight/agents", get(api_insight_agents))
        .route("/api/insight/agents/{agent_id}/profile", get(api_insight_agent_profile))
        .route("/api/insight/agents/{agent_id}/patterns", get(api_insight_agent_patterns))
        .route("/api/insight/runs", get(api_insight_runs))
        .route("/api/insight/runs/{run_id}", get(api_insight_run))
        .route("/api/insight/runs/{run_id}/graph", get(api_insight_graph))
        .route("/api/insight/runs/{run_id}/report", get(api_insight_report))
        .route("/api/insight/runs/merge", post(api_insight_merge_runs))
        .route("/api/insight/runs/{run_id}/split", post(api_insight_split_run))
        .route("/api/insight/audit/{audit_id}/traffic", get(api_insight_audit_traffic))
        .route("/api/insight/daily/{date}", get(api_insight_daily))
        .route("/api/insight/daily/{date}/print", get(api_insight_daily_print))
        .route("/api/insight/daily/generate", post(api_insight_generate_daily))
        .route("/api/insight/reset", post(api_insight_reset))
}

#[derive(Deserialize)]
struct ResetInsightRequest {
    #[serde(default)]
    replay_from_traffic: bool,
    limit: Option<usize>,
}

async fn api_insight_reset(
    State(s): State<HttpState>,
    Json(req): Json<ResetInsightRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let app = s.app.clone();
    let replay = req.replay_from_traffic;
    let limit = req.limit.unwrap_or(5000);
    let result = tokio::task::spawn_blocking(move || {
        if replay {
            app.replay_from_traffic(limit)
                .map(|stats| serde_json::json!({ "replay": stats }))
        } else {
            app.reset_insight()
                .map(|reset| serde_json::json!({ "reset": reset }))
        }
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(result))
}

async fn api_insight_status(State(s): State<HttpState>) -> Json<serde_json::Value> {
    let cfg = s.app.config();
    let critic_llm = if cfg.insight.llm_critic || cfg.insight.llm_daily {
        let snap = s.app.snapshot();
        Some(crate::insight_llm::status_critic_group(
            &snap.router,
            &cfg.insight.critic_model_group,
        ))
    } else {
        None
    };
    let metrics = s.app.insight.metrics_snapshot();
    Json(serde_json::json!({
        "enabled": s.app.insight.enabled(),
        "config": cfg.insight,
        "traffic_bodies": cfg.logging.save_traffic_bodies,
        "needs_traffic": cfg.insight.require_traffic_bodies && !cfg.logging.save_traffic_bodies,
        "critic_llm": critic_llm,
        "metrics": metrics,
    }))
}

async fn api_insight_agents(
    State(s): State<HttpState>,
    Query(q): Query<AgentsQuery>,
) -> Json<serde_json::Value> {
    let store = s.app.insight.store();
    let limit = q.limit.unwrap_or(200).min(500);
    let date = q.date.as_deref().and_then(parse_insight_date);
    let agents = if let Some(date) = date {
        store.agents_on_date(date).unwrap_or_default()
    } else {
        store.list_agents(limit).unwrap_or_default()
    };
    let agents: Vec<_> = agents.into_iter().take(limit).collect();
    let token_totals = date
        .and_then(|d| store.agent_token_totals_on_date(d).ok())
        .unwrap_or_default();
    let agents_json: Vec<_> = agents
        .into_iter()
        .map(|a| {
            let daily_tokens = token_totals.get(&a.agent_id).copied().unwrap_or(0);
            serde_json::json!({
                "agent_id": a.agent_id,
                "display_name": a.display_name,
                "agent_type": a.agent_type,
                "system_hash": a.system_hash,
                "tools_json": a.tools_json,
                "first_seen": a.first_seen,
                "last_seen": a.last_seen,
                "daily_tokens": daily_tokens,
            })
        })
        .collect();
    Json(serde_json::json!({ "agents": agents_json }))
}

async fn api_insight_agent_profile(
    State(s): State<HttpState>,
    Path(agent_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let store = s.app.insight.store();
    let agent = store
        .get_agent(&agent_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let stats = store
        .agent_run_stats(&agent_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let profile = build_profile(&agent, stats);
    Ok(Json(serde_json::json!({ "profile": profile })))
}

async fn api_insight_agent_patterns(
    State(s): State<HttpState>,
    Path(agent_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let store = s.app.insight.store();
    if store
        .get_agent(&agent_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .is_none()
    {
        return Err(StatusCode::NOT_FOUND);
    }
    let sequences = store
        .list_action_sequences(&agent_id, 200)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let patterns = mine_patterns(&sequences);
    Ok(Json(serde_json::json!({ "patterns": patterns, "sample_runs": sequences.len() })))
}

async fn api_insight_runs(
    State(s): State<HttpState>,
    Query(q): Query<RunsQuery>,
) -> Json<serde_json::Value> {
    let store = s.app.insight.store();
    let limit = q.limit.unwrap_or(100).min(500);
    let mut runs = if let Some(date) = q.date.as_deref().and_then(parse_insight_date) {
        if let Some(agent_id) = q.agent_id.as_deref() {
            store.runs_for_agent_on_date(agent_id, date)
        } else {
            store.runs_on_date(date)
        }
        .unwrap_or_default()
    } else {
        store
            .list_runs(q.agent_id.as_deref(), limit)
            .unwrap_or_default()
    };
    if q.date.is_some() {
        runs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        runs.truncate(limit);
    }
    let run_ids: Vec<String> = runs.iter().map(|r| r.run_id.clone()).collect();
    let audit_map = store.audit_ids_map_for_runs(&run_ids).unwrap_or_default();
    let audit_ids: Vec<String> = audit_map
        .values()
        .flat_map(|ids| ids.iter().cloned())
        .collect();
    let audits = load_audits_for_ids(&s.app.storage, &audit_ids);
    let risks = risk_for_runs(&audits, &audit_map, &run_ids);
    let enriched: Vec<Value> = runs
        .into_iter()
        .map(|run| {
            let risk = risks.get(&run.run_id).cloned().unwrap_or_default();
            serde_json::json!({ "run": run, "risk": risk })
        })
        .collect();
    Json(serde_json::json!({ "runs": enriched }))
}

async fn api_insight_run(
    State(s): State<HttpState>,
    Path(run_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let store = s.app.insight.store();
    let run = store.get_run(&run_id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let run = run.ok_or(StatusCode::NOT_FOUND)?;
    let events = store.list_events(&run_id).unwrap_or_default();
    let audit_ids = store.audit_ids_for_run(&run_id).unwrap_or_default();
    let audits = load_audits_for_ids(&s.app.storage, &audit_ids);
    let risk = risk_for_run(&store, &audits, &run_id);
    Ok(Json(serde_json::json!({ "run": run, "events": events, "risk": risk })))
}

async fn api_insight_graph(
    State(s): State<HttpState>,
    Path(run_id): Path<String>,
) -> Result<Response, StatusCode> {
    let store = s.app.insight.store();
    if store.get_run(&run_id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?.is_none() {
        return Err(StatusCode::NOT_FOUND);
    }
    let events = store
        .list_events(&run_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if events.is_empty() {
        return Err(StatusCode::NOT_FOUND);
    }
    let graph = smr_insight::graph::build_graph(&run_id, &events);
    Ok(Json(serde_json::to_value(graph).unwrap_or(Value::Null)).into_response())
}

async fn api_insight_report(
    State(s): State<HttpState>,
    Path(run_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let llm_critic = s.app.config().insight.llm_critic;
    let store = s.app.insight.store();
    let run = store
        .get_run(&run_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let prior = store
        .get_report(&run_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let events = store
        .list_events(&run_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let last_activity = smr_insight::report::last_activity_at(&run, &events);
    let status = smr_insight::report::reflection_report_status(
        &run,
        events.len(),
        prior.as_ref(),
        last_activity,
        llm_critic,
    );

    let displayable = prior
        .as_ref()
        .filter(|p| smr_insight::report::report_is_displayable(p, llm_critic));

    if displayable.is_none() {
        return Ok(Json(serde_json::json!({
            "report": null,
            "availability": status.availability,
            "next_llm_turn": status.next_llm_turn,
            "turn_count": status.turn_count,
            "event_count": status.event_count,
            "run": {
                "run_id": run.run_id,
                "goal": run.goal,
                "status": run.status,
                "turn_count": run.turn_count,
            },
        })));
    }

    let report = displayable.unwrap();
    let run_actions: Vec<String> = events
        .iter()
        .filter(|e| e.kind == EventKind::Action)
        .map(|e| e.summary.clone())
        .collect();

    let sequences = store
        .list_action_sequences(&run.agent_id, 200)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let sample_runs = sequences.len();
    let action_patterns: Vec<serde_json::Value> = mine_patterns(&sequences)
        .into_iter()
        .map(|p| {
            let matched_run = pattern_matches_run(&p, &run_actions);
            serde_json::json!({
                "steps": p.steps,
                "success_count": p.success_count,
                "failure_count": p.failure_count,
                "outcome_hint": p.outcome_hint,
                "matched_run": matched_run,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "report": report,
        "action_patterns": action_patterns,
        "sample_runs": sample_runs,
    })))
}

async fn api_insight_merge_runs(
    State(s): State<HttpState>,
    Json(req): Json<MergeRunsRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let store = s.app.insight.store();
    store
        .merge_runs(&req.target_run_id, &req.source_run_ids)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let run = store
        .get_run(&req.target_run_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({ "run": run })))
}

async fn api_insight_split_run(
    State(s): State<HttpState>,
    Path(run_id): Path<String>,
    Json(req): Json<SplitRunRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let store = s.app.insight.store();
    let new_run_id = store
        .split_run(&run_id, req.after_seq)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let run = store
        .get_run(&new_run_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({ "new_run_id": new_run_id, "run": run })))
}

async fn api_insight_audit_traffic(
    State(s): State<HttpState>,
    Path(audit_id): Path<String>,
) -> Json<serde_json::Value> {
    let records = s.app.traffic.list_by_audit(&audit_id);
    Json(serde_json::json!({ "audit_id": audit_id, "records": records }))
}

async fn api_insight_daily(
    State(s): State<HttpState>,
    Path(date): Path<String>,
    Query(q): Query<DailyQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let llm_daily = s.app.config().insight.llm_daily;
    let store = s.app.insight.store();
    let parsed_date = NaiveDate::parse_from_str(&date, "%Y-%m-%d")
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let status = smr_insight::report::daily_report_status(&store, parsed_date, llm_daily)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let reports = store
        .get_daily_report(&date, q.agent_id.as_deref())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({
        "date": date,
        "reports": reports,
        "availability": status.availability,
        "run_count": status.run_count,
    })))
}

async fn api_insight_daily_print(
    State(s): State<HttpState>,
    Path(date): Path<String>,
    Query(q): Query<DailyQuery>,
) -> Result<Response, StatusCode> {
    let store = s.app.insight.store();
    let reports = store
        .get_daily_report(&date, q.agent_id.as_deref())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let title = format!("AgentMirror Daily Report — {date}");
    let html = daily_reports_html(&reports, &title);
    Ok((
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        Html(html),
    )
        .into_response())
}

async fn api_insight_generate_daily(
    State(s): State<HttpState>,
    Json(req): Json<GenerateDailyRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let date = if let Some(d) = req.date {
        NaiveDate::parse_from_str(&d, "%Y-%m-%d")
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?
    } else {
        chrono::Local::now().date_naive()
    };
    let app = s.app.clone();
    let date_str = date.to_string();
    let (outcome, report) =
        tokio::task::spawn_blocking(move || app.insight.generate_daily_for_date(date))
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let generated = if outcome == smr_insight::report::DailyGenerateOutcome::Generated {
        1
    } else {
        0
    };
    let reports = if let Some(report) = report {
        vec![report]
    } else {
        s.app
            .insight
            .store()
            .get_daily_report(&date_str, None)
            .unwrap_or_default()
    };
    Ok(Json(serde_json::json!({
        "date": date.to_string(),
        "outcome": outcome,
        "generated": generated,
        "reports": reports,
    })))
}
