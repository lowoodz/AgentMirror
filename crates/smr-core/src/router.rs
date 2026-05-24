use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use http::{HeaderMap, HeaderValue, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use smr_protocol::ApiProtocol;
use tracing::{info, warn};

use crate::config::{AppConfig, ModelEndpoint};
use crate::request::ForwardRequest;

type HttpClient = Client<HttpConnector, Full<Bytes>>;

pub struct Router {
    config: Arc<AppConfig>,
    client: HttpClient,
}

#[derive(Debug)]
pub struct RouteAttempt {
    pub endpoint: ModelEndpoint,
    pub status: StatusCode,
    pub body: Bytes,
    pub headers: HeaderMap,
}

impl Router {
    pub fn new(config: Arc<AppConfig>) -> Self {
        let client = Client::builder(TokioExecutor::new()).build_http();
        Self { config, client }
    }

    pub fn resolve_group<'a>(&'a self, group_name: Option<&str>) -> Result<&'a [ModelEndpoint]> {
        let name = group_name.unwrap_or(&self.config.server.default_fallback_group);
        self.config
            .fallback_groups
            .get(name)
            .map(|v| v.as_slice())
            .ok_or_else(|| anyhow!("unknown fallback group: {name}"))
    }

    pub async fn forward_with_fallback(
        &self,
        group: &[ModelEndpoint],
        req: ForwardRequest<'_>,
    ) -> Result<RouteAttempt> {
        let mut last_error = None;

        for (idx, endpoint) in group.iter().enumerate() {
            match self.forward_once(endpoint, &req).await {
                Ok(attempt) if should_fallback(attempt.status) => {
                    warn!(
                        model = %endpoint.model,
                        status = %attempt.status,
                        attempt = idx + 1,
                        "fallback triggered"
                    );
                    last_error = Some(attempt);
                }
                Ok(attempt) => {
                    info!(model = %endpoint.model, status = %attempt.status, "request routed");
                    return Ok(attempt);
                }
                Err(err) => {
                    warn!(model = %endpoint.model, error = %err, attempt = idx + 1, "request failed");
                    last_error = Some(RouteAttempt {
                        endpoint: endpoint.clone(),
                        status: StatusCode::BAD_GATEWAY,
                        body: Bytes::from(err.to_string()),
                        headers: HeaderMap::new(),
                    });
                }
            }
        }

        last_error.ok_or_else(|| anyhow!("no endpoints configured in fallback group"))
    }

    async fn forward_once(
        &self,
        endpoint: &ModelEndpoint,
        req: &ForwardRequest<'_>,
    ) -> Result<RouteAttempt> {
        let base = endpoint.base_url.trim_end_matches('/');
        let mut url = format!("{base}{}", req.path);
        if let Some(q) = req.query {
            if !q.is_empty() {
                url.push('?');
                url.push_str(q);
            }
        }

        let uri: hyper::Uri = url.parse().context("invalid upstream url")?;
        let body = patch_model_in_body(req.body.clone(), &endpoint.model)?;

        let mut request = Request::builder()
            .method(req.method.clone())
            .uri(uri)
            .body(Full::new(body))?;

        copy_forward_headers(&req.headers, request.headers_mut(), endpoint, req.protocol)?;

        let response = tokio::time::timeout(
            Duration::from_secs(endpoint.timeout_secs),
            self.client.request(request),
        )
        .await
        .context("upstream timeout")??;

        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .into_body()
            .collect()
            .await
            .context("read upstream body")?
            .to_bytes();

        Ok(RouteAttempt {
            endpoint: endpoint.clone(),
            status,
            body,
            headers,
        })
    }
}

fn should_fallback(status: StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504)
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
