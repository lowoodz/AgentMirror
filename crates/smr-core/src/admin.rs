use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::routing::{get, put};
use axum::Router;
use serde::{Deserialize, Serialize};

use crate::config::AppConfig;
use crate::http_state::HttpState;

#[derive(Serialize)]
struct StatusResponse {
    listen: String,
    default_group: String,
    security_enabled: bool,
    dlp_enabled: bool,
    operation_mode: String,
    config_path: String,
    proxy_url: String,
    file_index_ready: bool,
    save_traffic_bodies: bool,
}

#[derive(Deserialize)]
struct EventsQuery {
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct AuditsQuery {
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct TrafficQuery {
    limit: Option<usize>,
}

pub fn router() -> Router<HttpState> {
    Router::new()
        .route("/ui", get(ui_index))
        .route("/favicon.ico", get(favicon))
        .route("/api/status", get(api_status))
        .route("/api/config", get(api_get_config).put(api_put_config))
        .route("/api/events", get(api_events))
        .route("/api/audits", get(api_audits))
        .route("/api/traffic", get(api_traffic))
        .route("/api/traffic/{id}", get(api_traffic_body))
        .route("/api/reload", put(api_reload))
}

async fn ui_index() -> Html<&'static str> {
    Html(include_str!("../assets/index.html"))
}

async fn favicon() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "image/png")],
        include_bytes!("../assets/favicon.png").as_slice(),
    )
}

async fn api_status(State(s): State<HttpState>) -> Json<StatusResponse> {
    let snap = s.app.snapshot();
    let cfg = snap.config;
    Json(StatusResponse {
        listen: cfg.server.listen.clone(),
        default_group: cfg.server.default_fallback_group.clone(),
        security_enabled: cfg.pipeline.security_enabled,
        dlp_enabled: cfg.pipeline.dlp_enabled,
        operation_mode: format!("{:?}", cfg.pipeline.operation_security_mode).to_lowercase(),
        config_path: s.app.config_path.display().to_string(),
        proxy_url: format!("http://{}/v1", cfg.server.listen),
        file_index_ready: snap.dlp.is_file_index_ready(),
        save_traffic_bodies: cfg.logging.save_traffic_bodies,
    })
}

async fn api_traffic(
    State(s): State<HttpState>,
    Query(q): Query<TrafficQuery>,
) -> Json<serde_json::Value> {
    let limit = q.limit.unwrap_or(30).min(100);
    Json(serde_json::json!({
        "records": s.app.traffic.list(limit),
        "enabled": s.app.config().logging.save_traffic_bodies,
        "traffic_dir": s.app.traffic.traffic_dir().display().to_string(),
    }))
}

async fn api_traffic_body(
    State(s): State<HttpState>,
    Path(id): Path<String>,
) -> Result<Response, StatusCode> {
    let (record, data) = s
        .app
        .traffic
        .read_body(&id)
        .ok_or(StatusCode::NOT_FOUND)?;
    let mut resp = Response::new(data.into());
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/octet-stream"),
    );
    resp.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        header::HeaderValue::from_str(&format!(
            "inline; filename=\"{}_{}.body\"",
            sanitize_download_name(&record.phase),
            &record.id[..8]
        ))
        .unwrap_or_else(|_| header::HeaderValue::from_static("inline")),
    );
    Ok(resp)
}

fn sanitize_download_name(phase: &str) -> String {
    phase
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

async fn api_get_config(State(s): State<HttpState>) -> Json<AppConfig> {
    Json(s.app.config())
}

async fn api_put_config(
    State(s): State<HttpState>,
    Json(config): Json<AppConfig>,
) -> Result<StatusCode, (StatusCode, String)> {
    s.app
        .save_config(&config)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::OK)
}

async fn api_events(
    State(s): State<HttpState>,
    Query(q): Query<EventsQuery>,
) -> Json<serde_json::Value> {
    let limit = q.limit.unwrap_or(50).min(200);
    Json(serde_json::json!({ "events": s.app.events.list(limit) }))
}

async fn api_audits(
    State(s): State<HttpState>,
    Query(q): Query<AuditsQuery>,
) -> Json<serde_json::Value> {
    let limit = q.limit.unwrap_or(50).min(200);
    let audits = s.app.storage.list_audits(limit).unwrap_or_default();
    Json(serde_json::json!({ "audits": audits }))
}

async fn api_reload(State(s): State<HttpState>) -> Result<StatusCode, (StatusCode, String)> {
    s.app
        .reload()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::OK)
}
