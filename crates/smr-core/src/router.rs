use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use http::{HeaderMap, HeaderValue, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use serde_json::Value;
use smr_protocol::{convert_body, ApiProtocol};
use tracing::{info, warn};

use crate::config::{AppConfig, ModelEndpoint};
use crate::request::ForwardRequest;
use crate::sse_stream::{collect_sse_for_routing, SseCollectResult, SsePassthroughStream};

type HttpClient = Client<hyper_rustls::HttpsConnector<HttpConnector>, Full<Bytes>>;

fn build_http_client() -> HttpClient {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
    let mut http = HttpConnector::new();
    http.enforce_http(false);
    // Embedded Mozilla roots — reliable on Windows portable builds (native store can fail).
    let https = HttpsConnectorBuilder::new()
        .with_webpki_roots()
        .https_or_http()
        .enable_http1()
        .build();
    Client::builder(TokioExecutor::new()).build(https)
}

pub struct Router {
    config: Arc<AppConfig>,
    client: HttpClient,
}

#[derive(Debug, Clone)]
pub struct ForwardOptions {
    pub wants_stream: bool,
    pub client_protocol: ApiProtocol,
}

impl Default for ForwardOptions {
    fn default() -> Self {
        Self {
            wants_stream: false,
            client_protocol: ApiProtocol::OpenAi,
        }
    }
}

pub enum RouteBody {
    Buffered(Bytes),
    SseStream(SsePassthroughStream),
}

impl std::fmt::Debug for RouteBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RouteBody::Buffered(b) => f.debug_tuple("Buffered").field(b).finish(),
            RouteBody::SseStream(_) => f.debug_tuple("SseStream").field(&"<stream>").finish(),
        }
    }
}

#[derive(Debug)]
pub struct RouteAttempt {
    pub endpoint: ModelEndpoint,
    pub status: StatusCode,
    pub body: RouteBody,
    pub headers: HeaderMap,
}

impl RouteAttempt {
    pub fn body_bytes(&self) -> Option<&Bytes> {
        match &self.body {
            RouteBody::Buffered(b) => Some(b),
            RouteBody::SseStream(_) => None,
        }
    }
}

#[derive(Debug)]
pub struct RouteResult {
    pub attempt: RouteAttempt,
    pub fallback_chain: Vec<String>,
    pub group_name: String,
}

impl Router {
    pub fn new(config: Arc<AppConfig>) -> Self {
        let client = build_http_client();
        Self { config, client }
    }

    pub fn resolve_group(&self, group_name: Option<&str>) -> Result<(String, Vec<ModelEndpoint>)> {
        let name = group_name
            .unwrap_or(&self.config.server.default_fallback_group)
            .to_string();
        self.config
            .fallback_groups
            .get(&name)
            .cloned()
            .map(|v| (name.clone(), v))
            .ok_or_else(|| anyhow!("unknown fallback group: {name}"))
    }

    pub async fn forward_with_fallback(
        &self,
        group_name: &str,
        group: &[ModelEndpoint],
        req: ForwardRequest<'_>,
        opts: ForwardOptions,
    ) -> Result<RouteResult> {
        let mut last_error: Option<RouteAttempt> = None;
        let mut chain: Vec<String> = Vec::new();

        for (idx, endpoint) in group.iter().enumerate() {
            chain.push(endpoint.model.clone());
            match self.forward_once(endpoint, &req, opts.wants_stream).await {
                Ok(attempt) if should_fallback_status(attempt.status) => {
                    warn!(
                        model = %endpoint.model,
                        status = %attempt.status,
                        attempt = idx + 1,
                        "fallback triggered (status)"
                    );
                    last_error = Some(attempt);
                }
                Ok(attempt)
                    if attempt.body_bytes().is_some_and(|body| {
                        is_malformed_success(&attempt.status, &attempt.headers, body)
                    }) =>
                {
                    warn!(model = %endpoint.model, "fallback triggered (malformed response)");
                    last_error = Some(attempt);
                }
                Ok(attempt)
                    if opts.wants_stream
                        && is_sse(&attempt.headers)
                        && matches!(attempt.body, RouteBody::Buffered(ref b) if !sse_has_first_token(b)) =>
                {
                    warn!(model = %endpoint.model, "fallback triggered (stream: no first token)");
                    last_error = Some(attempt);
                }
                Ok(attempt) => {
                    info!(model = %endpoint.model, status = %attempt.status, "request routed");
                    return Ok(RouteResult {
                        attempt,
                        fallback_chain: chain,
                        group_name: group_name.to_string(),
                    });
                }
                Err(err) => {
                    warn!(model = %endpoint.model, error = %err, attempt = idx + 1, "request failed");
                    last_error = Some(RouteAttempt {
                        endpoint: endpoint.clone(),
                        status: StatusCode::BAD_GATEWAY,
                        body: RouteBody::Buffered(Bytes::from(err.to_string())),
                        headers: HeaderMap::new(),
                    });
                }
            }
        }

        if let Some(last) = last_error {
            let msg = format!(
                "SafeRoute: fallback group '{}' exhausted — tried {} endpoint(s): {}. Last status: {}.",
                group_name,
                chain.len(),
                chain.join(" → "),
                last.status
            );
            return Ok(RouteResult {
                attempt: RouteAttempt {
                    endpoint: last.endpoint,
                    status: StatusCode::BAD_GATEWAY,
                    body: RouteBody::Buffered(Bytes::from(msg)),
                    headers: HeaderMap::new(),
                },
                fallback_chain: chain,
                group_name: group_name.to_string(),
            });
        }

        Err(anyhow!("no endpoints configured in fallback group '{group_name}'"))
    }

    async fn forward_once(
        &self,
        endpoint: &ModelEndpoint,
        req: &ForwardRequest<'_>,
        wants_stream: bool,
    ) -> Result<RouteAttempt> {
        let target_protocol = endpoint.resolve_protocol();
        let mut path = req.path.to_string();
        let mut body = patch_model_in_body(req.body.clone(), &endpoint.model)?;

        if req.protocol != target_protocol && !body.is_empty() {
            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&body) {
                path = smr_protocol::target_path(&path, target_protocol == ApiProtocol::Anthropic);
                let converted = convert_body(&json, req.protocol, target_protocol);
                body = Bytes::from(serde_json::to_vec(&converted)?);
                if req.protocol != target_protocol {
                    info!(
                        model = %endpoint.model,
                        from = ?req.protocol,
                        to = ?target_protocol,
                        "converting request via unified body"
                    );
                }
            }
        }

        let base = endpoint.base_url.trim_end_matches('/');
        path = normalize_upstream_path(base, &path);
        let mut url = format!("{base}{path}");
        if let Some(q) = req.query {
            if !q.is_empty() {
                url.push('?');
                url.push_str(q);
            }
        }

        let uri: hyper::Uri = url.parse().context("invalid upstream url")?;

        let mut request = Request::builder()
            .method(req.method.clone())
            .uri(uri)
            .body(Full::new(body))?;

        copy_forward_headers(&req.headers, request.headers_mut(), endpoint, target_protocol)?;

        let response = tokio::time::timeout(
            Duration::from_secs(endpoint.timeout_secs),
            self.client.request(request),
        )
        .await
        .context("upstream timeout")??;

        let status = response.status();
        let headers = response.headers().clone();
        let incoming = response.into_body();

        let route_body = if wants_stream && status.is_success() && is_sse(&headers) {
            match collect_sse_for_routing(incoming).await? {
                SseCollectResult::Passthrough { prefix, rest } => {
                    RouteBody::SseStream(SsePassthroughStream::new(prefix, rest))
                }
                SseCollectResult::Buffered(bytes) => RouteBody::Buffered(bytes),
                SseCollectResult::NoFirstToken(bytes) => RouteBody::Buffered(bytes),
            }
        } else {
            let body = incoming
                .collect()
                .await
                .context("read upstream body")?
                .to_bytes();
            RouteBody::Buffered(body)
        };

        Ok(RouteAttempt {
            endpoint: endpoint.clone(),
            status,
            body: route_body,
            headers,
        })
    }
}

pub fn convert_response_body(
    body: &Bytes,
    from: ApiProtocol,
    to: ApiProtocol,
) -> Result<Bytes> {
    if from == to || body.is_empty() {
        return Ok(body.clone());
    }
    let json: Value = serde_json::from_slice(body).context("parse response json")?;
    let converted = match (from, to) {
        (ApiProtocol::Anthropic, ApiProtocol::OpenAi) => {
            smr_protocol::anthropic_response_to_openai(&json)
        }
        (ApiProtocol::OpenAi, ApiProtocol::Anthropic) => {
            smr_protocol::openai_response_to_anthropic(&json)
        }
        _ => json,
    };
    Ok(Bytes::from(serde_json::to_vec(&converted)?))
}

fn should_fallback_status(status: StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504)
}

fn is_malformed_success(status: &StatusCode, headers: &HeaderMap, body: &Bytes) -> bool {
    if !status.is_success() || body.is_empty() {
        return false;
    }
    let is_json = headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("application/json"))
        .unwrap_or(false);
    if is_json {
        return serde_json::from_slice::<Value>(body).is_err();
    }
    false
}

fn is_sse(headers: &HeaderMap) -> bool {
    headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("text/event-stream"))
        .unwrap_or(false)
}

pub fn sse_has_first_token(body: &[u8]) -> bool {
    for line in String::from_utf8_lossy(body).lines() {
        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };
        let trimmed = data.trim();
        if trimmed.is_empty() || trimmed == "[DONE]" {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
            if sse_chunk_has_content(&v) {
                return true;
            }
        }
    }
    false
}

fn sse_chunk_has_content(v: &Value) -> bool {
    if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
        for c in choices {
            if c.get("delta")
                .and_then(|d| d.get("content"))
                .and_then(|t| t.as_str())
                .is_some_and(|s| !s.is_empty())
            {
                return true;
            }
            if c.get("delta").and_then(|d| d.get("tool_calls")).is_some() {
                return true;
            }
        }
    }
    if v.get("type").and_then(|t| t.as_str()) == Some("content_block_delta") {
        return true;
    }
    if v.get("delta")
        .and_then(|d| d.get("text"))
        .and_then(|t| t.as_str())
        .is_some_and(|s| !s.is_empty())
    {
        return true;
    }
    false
}

fn normalize_upstream_path(base: &str, path: &str) -> String {
    // GLM coding API: .../v4 + /v1/chat/completions -> .../v4/chat/completions
    if base.ends_with("/v4") && path.starts_with("/v1/") {
        return path.replacen("/v1", "", 1);
    }
    path.to_string()
}

fn patch_model_in_body(body: Bytes, model: &str) -> Result<Bytes> {
    if body.is_empty() {
        return Ok(body);
    }
    let mut json: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return Ok(body),
    };
    if json.get("model").is_some() {
        json["model"] = serde_json::Value::String(model.to_string());
        return Ok(Bytes::from(serde_json::to_vec(&json)?));
    }
    Ok(body)
}

fn copy_forward_headers(
    incoming: &HeaderMap,
    outgoing: &mut HeaderMap,
    endpoint: &ModelEndpoint,
    protocol: ApiProtocol,
) -> Result<()> {
    for (name, value) in incoming.iter() {
        let n = name.as_str().to_ascii_lowercase();
        if n == "host" || n == "content-length" || n == "connection" || n.starts_with("x-smr-") {
            continue;
        }
        outgoing.insert(name.clone(), value.clone());
    }

    if let Some(key) = endpoint.resolve_api_key() {
        match protocol {
            ApiProtocol::Anthropic => {
                outgoing.insert("x-api-key", HeaderValue::from_str(&key)?);
                if !outgoing.contains_key("anthropic-version") {
                    outgoing.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
                }
            }
            ApiProtocol::OpenAi => {
                outgoing.insert(
                    "authorization",
                    HeaderValue::from_str(&format!("Bearer {key}"))?,
                );
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_sse_first_token() {
        let body = b"data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n";
        assert!(sse_has_first_token(body));
    }

    #[test]
    fn no_token_in_empty_sse() {
        let body = b"data: [DONE]\n\n";
        assert!(!sse_has_first_token(body));
    }

    #[test]
    fn normalizes_v4_openai_path() {
        let path = normalize_upstream_path(
            "https://open.bigmodel.cn/api/coding/paas/v4",
            "/v1/chat/completions",
        );
        assert_eq!(path, "/chat/completions");
    }
}
