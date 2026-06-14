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
use smr_insight::pattern::mine_patterns;
use smr_insight::profile::build_profile;

#[derive(Deserialize)]
struct RunsQuery {
    agent_id: Option<String>,
    limit: Option<usize>,
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

#[derive(Deserialize)]
struct PatchRunRequest {
    goal: Option<String>,
}

pub fn router() -> Router<HttpState> {
    Router::new()
        .route("/api/insight/status", get(api_insight_status))
        .route("/api/insight/agents", get(api_insight_agents))
        .route("/api/insight/agents/{agent_id}/profile", get(api_insight_agent_profile))
        .route("/api/insight/agents/{agent_id}/patterns", get(api_insight_agent_patterns))
        .route("/api/insight/runs", get(api_insight_runs))
        .route("/api/insight/runs/{run_id}", get(api_insight_run).patch(api_insight_patch_run))
        .route("/api/insight/runs/{run_id}/graph", get(api_insight_graph))
        .route("/api/insight/runs/{run_id}/report", get(api_insight_report))
        .route("/api/insight/runs/merge", post(api_insight_merge_runs))
        .route("/api/insight/runs/{run_id}/split", post(api_insight_split_run))
        .route("/api/insight/audit/{audit_id}/traffic", get(api_insight_audit_traffic))
        .route("/api/insight/daily/{date}", get(api_insight_daily))
        .route("/api/insight/daily/{date}/print", get(api_insight_daily_print))
        .route("/api/insight/daily/generate", post(api_insight_generate_daily))
}

async fn api_insight_status(State(s): State<HttpState>) -> Json<serde_json::Value> {
    let cfg = s.app.config();
    Json(serde_json::json!({
        "enabled": s.app.insight.enabled(),
        "config": cfg.insight,
        "traffic_bodies": cfg.logging.save_traffic_bodies,
        "needs_traffic": cfg.insight.require_traffic_bodies && !cfg.logging.save_traffic_bodies,
    }))
}

async fn api_insight_agents(State(s): State<HttpState>) -> Json<serde_json::Value> {
    let store = s.app.insight.store();
    let agents = store.list_agents(200).unwrap_or_default();
    Json(serde_json::json!({ "agents": agents }))
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
    let runs = store
        .list_runs(q.agent_id.as_deref(), limit)
        .unwrap_or_default();
    let run_ids: Vec<String> = runs.iter().map(|r| r.run_id.clone()).collect();
    let audit_ids = store.audit_ids_for_runs(&run_ids).unwrap_or_default();
    let audits = load_audits_for_ids(&s.app.storage, &audit_ids);
    let risks = risk_for_runs(&store, &audits, &run_ids);
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

async fn api_insight_patch_run(
    State(s): State<HttpState>,
    Path(run_id): Path<String>,
    Json(req): Json<PatchRunRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let store = s.app.insight.store();
    if store.get_run(&run_id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?.is_none() {
        return Err(StatusCode::NOT_FOUND);
    }
    if let Some(goal) = req.goal.filter(|g| !g.trim().is_empty()) {
        store
            .update_run_goal(&run_id, goal.trim())
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }
    let run = store.get_run(&run_id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({ "run": run })))
}

async fn api_insight_graph(
    State(s): State<HttpState>,
    Path(run_id): Path<String>,
) -> Result<Response, StatusCode> {
    let store = s.app.insight.store();
    if store.get_run(&run_id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?.is_none() {
        return Err(StatusCode::NOT_FOUND);
    }
    let graph = store
        .load_graph_json(&run_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    match graph {
        Some(text) => {
            let v: serde_json::Value =
                serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
            Ok(Json(v).into_response())
        }
        None => Err(StatusCode::NOT_FOUND),
    }
}

async fn api_insight_report(
    State(s): State<HttpState>,
    Path(run_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let store = s.app.insight.store();
    let report = store
        .get_report(&run_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    match report {
        Some(r) => Ok(Json(serde_json::json!({ "report": r }))),
        None => Err(StatusCode::NOT_FOUND),
    }
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
    let store = s.app.insight.store();
    let reports = store
        .get_daily_report(&date, q.agent_id.as_deref())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({ "date": date, "reports": reports })))
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
    let count = tokio::task::spawn_blocking(move || app.insight.generate_daily_for_date(date))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "date": date.to_string(), "generated": count })))
}
