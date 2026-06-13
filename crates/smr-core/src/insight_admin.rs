use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::NaiveDate;
use serde::Deserialize;

use crate::http_state::HttpState;

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

pub fn router() -> Router<HttpState> {
    Router::new()
        .route("/api/insight/status", get(api_insight_status))
        .route("/api/insight/agents", get(api_insight_agents))
        .route("/api/insight/runs", get(api_insight_runs))
        .route("/api/insight/runs/{run_id}", get(api_insight_run))
        .route("/api/insight/runs/{run_id}/graph", get(api_insight_graph))
        .route("/api/insight/runs/{run_id}/report", get(api_insight_report))
        .route("/api/insight/daily/{date}", get(api_insight_daily))
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

async fn api_insight_runs(
    State(s): State<HttpState>,
    Query(q): Query<RunsQuery>,
) -> Json<serde_json::Value> {
    let store = s.app.insight.store();
    let limit = q.limit.unwrap_or(100).min(500);
    let runs = store
        .list_runs(q.agent_id.as_deref(), limit)
        .unwrap_or_default();
    Json(serde_json::json!({ "runs": runs }))
}

async fn api_insight_run(
    State(s): State<HttpState>,
    Path(run_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let store = s.app.insight.store();
    let run = store.get_run(&run_id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let run = run.ok_or(StatusCode::NOT_FOUND)?;
    let events = store.list_events(&run_id).unwrap_or_default();
    Ok(Json(serde_json::json!({ "run": run, "events": events })))
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
