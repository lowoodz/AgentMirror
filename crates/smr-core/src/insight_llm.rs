use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::Method;
use serde_json::{json, Value};
use smr_insight::LlmClient;
use smr_protocol::{detect_protocol, ApiProtocol};

use crate::provider;
use crate::proxy_path::PATH_CHAT_COMPLETIONS;
use crate::request::ForwardRequest;
use crate::router::{convert_response_body, ForwardOptions, Router, RouteBody};

const LIVE_PROBE_TTL: Duration = Duration::from_secs(300);

struct LiveProbeCache {
    group: String,
    result: serde_json::Value,
    at: Instant,
}

static LIVE_PROBE_CACHE: Mutex<Option<LiveProbeCache>> = Mutex::new(None);

struct InsightLlmJob {
    router: Arc<Router>,
    group: String,
    system: String,
    user: String,
    reply: mpsc::Sender<anyhow::Result<String>>,
}

struct InsightLlmRuntime {
    tx: mpsc::Sender<InsightLlmJob>,
}

static INSIGHT_LLM_RUNTIME: OnceLock<InsightLlmRuntime> = OnceLock::new();

fn insight_llm_runtime() -> &'static InsightLlmRuntime {
    INSIGHT_LLM_RUNTIME.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<InsightLlmJob>();
        std::thread::Builder::new()
            .name("insight-llm".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("insight LLM runtime");
                while let Ok(job) = rx.recv() {
                    let result = rt.block_on(async {
                        complete_async(&job.router, &job.group, &job.system, &job.user).await
                    });
                    let _ = job.reply.send(result);
                }
            })
            .expect("spawn insight LLM thread");
        InsightLlmRuntime { tx }
    })
}

pub struct RouterLlmClient {
    router: Arc<Router>,
    group: String,
}

impl RouterLlmClient {
    pub fn new(router: Arc<Router>, group: &str) -> Self {
        Self {
            router,
            group: group.to_string(),
        }
    }
}

impl LlmClient for RouterLlmClient {
    fn complete(&self, system: &str, user: &str) -> anyhow::Result<String> {
        let (reply_tx, reply_rx) = mpsc::channel();
        insight_llm_runtime()
            .tx
            .send(InsightLlmJob {
                router: Arc::clone(&self.router),
                group: self.group.clone(),
                system: system.to_string(),
                user: user.to_string(),
                reply: reply_tx,
            })
            .map_err(|e| anyhow::anyhow!("insight LLM worker unavailable: {e}"))?;
        reply_rx
            .recv()
            .map_err(|_| anyhow::anyhow!("insight LLM worker dropped reply"))?
    }
}

/// Config/routing readiness for AgentMirror critic (no upstream LLM call).
pub fn critic_group_readiness(router: &Router, group: &str) -> serde_json::Value {
    match router.resolve_group(Some(group)) {
        Ok((group_name, endpoints)) => {
            if endpoints.is_empty() {
                return serde_json::json!({
                    "ok": false,
                    "group": group,
                    "error": format!("no models in critic group '{group}'"),
                });
            }
            let with_key: Vec<_> = endpoints
                .iter()
                .filter(|e| e.resolve_api_key().is_some())
                .map(|e| {
                    serde_json::json!({
                        "id": e.id,
                        "model": e.model,
                        "protocol": format!("{:?}", e.resolve_protocol()),
                    })
                })
                .collect();
            if with_key.is_empty() {
                return serde_json::json!({
                    "ok": false,
                    "group": group,
                    "error": "critic group has no endpoints with API keys",
                });
            }
            serde_json::json!({
                "ok": true,
                "group": group_name,
                "endpoints": with_key,
            })
        }
        Err(err) => serde_json::json!({
            "ok": false,
            "group": group,
            "error": err.to_string(),
        }),
    }
}

/// Status API probe: instant readiness + optional cached live LLM ping (≤1 per 5 min).
pub fn probe_critic_group(router: Arc<Router>, group: &str) -> serde_json::Value {
    let readiness = critic_group_readiness(&router, group);
    let ready = readiness.get("ok").and_then(|v| v.as_bool()) == Some(true);

    let live = live_probe_cached(router, group);
    let live_ok = live.as_ref().and_then(|l| l.get("ok")).and_then(|v| v.as_bool());

    let mut out = serde_json::json!({
        "ok": ready && live_ok.unwrap_or(true),
        "group": group,
        "ready": readiness,
    });
    if let Some(live) = live {
        out["live_probe"] = live;
    }
    if !ready {
        if let Some(err) = readiness.get("error") {
            out["error"] = err.clone();
        }
    } else if live_ok == Some(false) {
        if let Some(live) = out.get("live_probe").and_then(|l| l.get("error")) {
            out["error"] = live.clone();
        }
    }
    out
}

fn live_probe_cached(router: Arc<Router>, group: &str) -> Option<serde_json::Value> {
    {
        let cache = LIVE_PROBE_CACHE.lock().ok()?;
        if let Some(entry) = cache.as_ref() {
            if entry.group == group && entry.at.elapsed() < LIVE_PROBE_TTL {
                let mut result = entry.result.clone();
                if let Some(obj) = result.as_object_mut() {
                    obj.insert("cached".into(), serde_json::json!(true));
                }
                return Some(result);
            }
        }
    }

    let result = run_live_probe(router, group);
    if let Ok(mut cache) = LIVE_PROBE_CACHE.lock() {
        *cache = Some(LiveProbeCache {
            group: group.to_string(),
            result: result.clone(),
            at: Instant::now(),
        });
    }
    Some(result)
}

fn run_live_probe(router: Arc<Router>, group: &str) -> serde_json::Value {
    let client = RouterLlmClient::new(router, group);
    match client.complete(
        r#"Reply with JSON only: {"ok":true}"#,
        "AgentMirror critic probe",
    ) {
        Ok(_) => serde_json::json!({ "ok": true, "cached": false }),
        Err(err) => serde_json::json!({ "ok": false, "cached": false, "error": err.to_string() }),
    }
}

async fn complete_async(
    router: &Router,
    group: &str,
    system: &str,
    user: &str,
) -> anyhow::Result<String> {
    let (group_name, endpoints) = router.resolve_group(Some(group))?;
    if endpoints.is_empty() {
        anyhow::bail!("no models in insight critic group '{group}'");
    }
    let missing_key: Vec<String> = endpoints
        .iter()
        .filter(|e| e.resolve_api_key().is_none())
        .map(|e| e.id.clone())
        .collect();
    if missing_key.len() == endpoints.len() {
        anyhow::bail!(
            "critic group '{group}' has no API keys (check api_key or api_key_env on: {})",
            missing_key.join(", ")
        );
    }

    let public_model = provider::public_model_id(group);
    let body_json = json!({
        "model": public_model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user}
        ],
        "stream": false,
        "max_tokens": 8192,
        "temperature": 0.25
    });
    let headers = {
        let mut h = http::HeaderMap::new();
        h.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/json"),
        );
        h
    };
    let client_protocol = detect_protocol(PATH_CHAT_COMPLETIONS, &headers, &body_json);
    let req = ForwardRequest {
        method: Method::POST,
        path: PATH_CHAT_COMPLETIONS,
        query: None,
        headers,
        body: Bytes::from(serde_json::to_vec(&body_json)?),
        protocol: client_protocol,
    };
    let result = router
        .forward_with_fallback(
            &group_name,
            &endpoints,
            req,
            ForwardOptions {
                wants_stream: false,
                client_protocol,
                allow_cross_protocol: true,
            },
        )
        .await?;
    if !result.attempt.status.is_success() {
        anyhow::bail!("insight LLM upstream status {}", result.attempt.status);
    }
    let endpoint_protocol = result.attempt.endpoint.resolve_protocol();
    let text = match result.attempt.body {
        RouteBody::Buffered(bytes) => {
            let bytes = if client_protocol != endpoint_protocol {
                convert_response_body(&bytes, endpoint_protocol, client_protocol)?
            } else {
                bytes
            };
            extract_assistant_text(&bytes)
                .ok_or_else(|| anyhow::anyhow!("empty LLM response"))?
        }
        RouteBody::SseStream(_) => anyhow::bail!("unexpected SSE from insight LLM"),
    };
    Ok(text)
}

fn extract_assistant_text(body: &Bytes) -> Option<String> {
    let json: Value = serde_json::from_slice(body).ok()?;
    if let Some(text) = json
        .get("choices")?
        .as_array()?
        .first()?
        .get("message")?
        .get("content")?
        .as_str()
    {
        return Some(text.to_string());
    }
    json.get("content")?
        .as_array()?
        .iter()
        .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
        .collect::<Vec<_>>()
        .join("")
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_probe_cache_ttl_constant_is_reasonable() {
        assert_eq!(LIVE_PROBE_TTL, Duration::from_secs(300));
    }
}
