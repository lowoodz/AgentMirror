use std::sync::Arc;

use anyhow::Result;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{any, get};
use axum::Router;
use tower_http::trace::TraceLayer;
use tracing::info;
use crate::admin;
use crate::session_key;
use crate::body_util::{apply_streaming_headers, proxy_body_to_axum};
use crate::http_state::HttpState;
use crate::proxy::ProxyService;
use crate::proxy_path;
use crate::request::{ProxyBody, ProxyRequest};
use crate::state::SharedApp;

pub async fn run_app(app: Arc<SharedApp>) -> Result<()> {
    let listen = app.config().server.listen.clone();
    let proxy = Arc::new(ProxyService::new(app.clone()));
    let state = HttpState { app, proxy };

    let app = Router::new()
        .route("/health", get(health))
        .route("/", get(|| async { Redirect::permanent("/ui") }))
        .merge(admin::router())
        .fallback(any(proxy_handler))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr: std::net::SocketAddr = listen
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid listen address: {e}"))?;
    info!(%addr, ui = %format!("http://{addr}/ui"), "LLM-SafeRoute listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> impl IntoResponse {
    (
        StatusCode::OK,
        format!(
            "LLM-SafeRoute OK {} ui={}",
            crate::VERSION,
            crate::UI_DIGEST
        ),
    )
}

async fn proxy_handler(
    State(state): State<HttpState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path();
    if path.starts_with("/ui") || path.starts_with("/api/") {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }

    let session_id = session_key::derive_session_id(&headers, &body);

    let (path_tier, forward_path) = proxy_path::split_tier_path(path);
    let header_group = headers
        .get("x-smr-fallback-group")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let fallback_group = path_tier
        .map(|s| s.to_string())
        .or(header_group);

    let query_string = uri.query().map(|s| s.to_string());

    match state
        .proxy
        .handle_api_request(ProxyRequest {
            session_id: &session_id,
            fallback_group: fallback_group.as_deref(),
            method,
            path: &forward_path,
            query: query_string.as_deref(),
            headers,
            body,
        })
        .await
    {
        Ok((status, resp_headers, proxy_body)) => {
            let is_stream = matches!(proxy_body, ProxyBody::SseStream(_));
            let mut resp = Response::builder()
                .status(status)
                .body(proxy_body_to_axum(proxy_body))
                .unwrap_or_else(|_| {
                    (StatusCode::INTERNAL_SERVER_ERROR, "failed to build response").into_response()
                });
            {
                let h = resp.headers_mut();
                for (name, value) in resp_headers.iter() {
                    let n = name.as_str().to_ascii_lowercase();
                    if n == "transfer-encoding" || n == "content-length" {
                        continue;
                    }
                    h.insert(name.clone(), value.clone());
                }
                if is_stream {
                    apply_streaming_headers(h);
                }
            }
            resp
        }
        Err(err) => {
            tracing::error!(error = %err, "proxy error");
            state.app.events.push(
                crate::events::EventKind::Error,
                format!("proxy error: {err}"),
                None,
            );
            (
                StatusCode::BAD_GATEWAY,
                format!("LLM-SafeRoute proxy error: {err}"),
            )
                .into_response()
        }
    }
}
