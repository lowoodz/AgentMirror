use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, Json};
use axum::routing::{get, put};
use axum::Router;
use serde::{Deserialize, Serialize};

use crate::config::AppConfig;
use crate::http_state::HttpState;

#[derive(Serialize)]
struct StatusResponse {
    listen: String,
    default_group: String,
    dlp_enabled: bool,
    operation_mode: String,
    config_path: String,
    proxy_url: String,
}

#[derive(Deserialize)]
struct EventsQuery {
    limit: Option<usize>,
}

pub fn router() -> Router<HttpState> {
    Router::new()
        .route("/ui", get(ui_index))
        .route("/api/status", get(api_status))
        .route("/api/config", get(api_get_config).put(api_put_config))
        .route("/api/events", get(api_events))
        .route("/api/reload", put(api_reload))
}

async fn ui_index() -> Html<&'static str> {
    Html(include_str!("../assets/index.html"))
}

async fn api_status(State(s): State<HttpState>) -> Json<StatusResponse> {
    let cfg = s.app.config();
    Json(StatusResponse {
        listen: cfg.server.listen.clone(),
        default_group: cfg.server.default_fallback_group.clone(),
        dlp_enabled: cfg.pipeline.dlp_enabled,
        operation_mode: format!("{:?}", cfg.pipeline.operation_security_mode).to_lowercase(),
        config_path: s.app.config_path.display().to_string(),
        proxy_url: format!("http://{}/v1", cfg.server.listen),
    })
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

async fn api_reload(State(s): State<HttpState>) -> Result<StatusCode, (StatusCode, String)> {
    s.app
        .reload()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::OK)
}
