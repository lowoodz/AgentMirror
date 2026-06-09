use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::routing::{get, put};
use axum::Router;
use serde::{Deserialize, Serialize};

use crate::config::{AppConfig, TrafficRequestCapture};
use crate::http_state::HttpState;
use crate::provider::{self, PublicModelInfo};
use crate::proxy_path;

#[derive(Serialize)]
struct StatusResponse {
    listen: String,
    default_group: String,
    security_enabled: bool,
    dlp_enabled: bool,
    operation_mode: String,
    config_path: String,
    proxy_url: String,
    proxy_url_high: String,
    proxy_url_medium: String,
    proxy_url_lite: String,
    provider_id: String,
    provider_name: String,
    provider_url: String,
    models: Vec<PublicModelInfo>,
    file_index_ready: bool,
    file_index_rebuilding: bool,
    save_traffic_bodies: bool,
    traffic_request_capture: String,
}

#[derive(Deserialize)]
struct EventsQuery {
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct AuditsQuery {
    limit: Option<usize>,
    session_id: Option<String>,
}

#[derive(Deserialize)]
struct TrafficQuery {
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct TrafficBodyQuery {
    format: Option<String>,
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

async fn ui_index() -> impl IntoResponse {
    (
        [
            (header::CACHE_CONTROL, "no-store, no-cache, must-revalidate"),
            (header::PRAGMA, "no-cache"),
        ],
        Html(include_str!("../assets/index.html")),
    )
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
    let listen = cfg.server.listen.clone();
    let (proxy_url_high, proxy_url_medium, proxy_url_lite) = proxy_path::proxy_tier_urls(&listen);
    let provider_url = provider::provider_base_url(&listen);
    let models = provider::list_public_models(&cfg);
    Json(StatusResponse {
        listen: listen.clone(),
        default_group: cfg.server.default_fallback_group.clone(),
        security_enabled: cfg.pipeline.security_enabled,
        dlp_enabled: cfg.pipeline.dlp_enabled,
        operation_mode: format!("{:?}", cfg.pipeline.operation_security_mode).to_lowercase(),
        config_path: s.app.config_path.display().to_string(),
        proxy_url: provider_url.clone(),
        proxy_url_high,
        proxy_url_medium,
        proxy_url_lite,
        provider_id: provider::PROVIDER_ID.to_string(),
        provider_name: provider::PROVIDER_NAME.to_string(),
        provider_url,
        models,
        file_index_ready: snap.dlp.is_file_index_ready(),
        file_index_rebuilding: snap.dlp.is_file_index_rebuilding(),
        save_traffic_bodies: cfg.logging.save_traffic_bodies,
        traffic_request_capture: match cfg.logging.traffic_request_capture {
            TrafficRequestCapture::BeforeDlp => "before_dlp",
            TrafficRequestCapture::AfterDlp => "after_dlp",
        }
        .into(),
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
    Query(q): Query<TrafficBodyQuery>,
) -> Result<Response, StatusCode> {
    let (record, data) = s
        .app
        .traffic
        .read_body(&id)
        .ok_or(StatusCode::NOT_FOUND)?;

    if q.format.as_deref() == Some("json") {
        let text = String::from_utf8_lossy(&data).into_owned();
        let parsed = serde_json::from_slice::<serde_json::Value>(&data).ok();
        let body = serde_json::json!({
            "record": record,
            "text": text,
            "parsed": parsed,
            "is_json": parsed.is_some(),
        });
        return Ok(Json(body).into_response());
    }

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
    let app = s.app.clone();
    tokio::task::spawn_blocking(move || app.save_config(&config))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
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
    let audits = if let Some(session_id) = q.session_id.filter(|id| !id.is_empty()) {
        s.app
            .storage
            .list_audits_for_session(&session_id, limit)
            .unwrap_or_default()
    } else {
        s.app.storage.list_audits(limit).unwrap_or_default()
    };
    Json(serde_json::json!({ "audits": audits }))
}

async fn api_reload(State(s): State<HttpState>) -> Result<StatusCode, (StatusCode, String)> {
    let app = s.app.clone();
    tokio::task::spawn_blocking(move || app.reload())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::OK)
}
