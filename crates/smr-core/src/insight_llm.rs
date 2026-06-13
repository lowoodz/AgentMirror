use std::sync::Arc;

use bytes::Bytes;
use http::Method;
use serde_json::{json, Value};
use smr_insight::LlmClient;
use smr_protocol::ApiProtocol;

use crate::provider;
use crate::proxy_path::PATH_CHAT_COMPLETIONS;
use crate::request::ForwardRequest;
use crate::router::{ForwardOptions, Router, RouteBody};

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
        let router = Arc::clone(&self.router);
        let group = self.group.clone();
        let system = system.to_string();
        let user = user.to_string();

        let fut = async move {
            let (group_name, endpoints) = router.resolve_group(Some(&group))?;
            if endpoints.is_empty() {
                anyhow::bail!("no models in insight critic group '{group}'");
            }
            let public_model = provider::public_model_id(&group);
            let body = json!({
                "model": public_model,
                "messages": [
                    {"role": "system", "content": system},
                    {"role": "user", "content": user}
                ],
                "stream": false,
                "max_tokens": 1500,
                "temperature": 0.2
            });
            let req = ForwardRequest {
                method: Method::POST,
                path: PATH_CHAT_COMPLETIONS,
                query: None,
                headers: http::HeaderMap::new(),
                body: Bytes::from(serde_json::to_vec(&body)?),
                protocol: ApiProtocol::OpenAi,
            };
            let result = router
                .forward_with_fallback(
                    &group_name,
                    &endpoints,
                    req,
                    ForwardOptions {
                        wants_stream: false,
                        client_protocol: ApiProtocol::OpenAi,
                        allow_cross_protocol: true,
                    },
                )
                .await?;
            if !result.attempt.status.is_success() {
                anyhow::bail!("insight LLM upstream status {}", result.attempt.status);
            }
            let text = match result.attempt.body {
                RouteBody::Buffered(bytes) => extract_assistant_text(&bytes)
                    .ok_or_else(|| anyhow::anyhow!("empty LLM response"))?,
                RouteBody::SseStream(_) => anyhow::bail!("unexpected SSE from insight LLM"),
            };
            Ok(text)
        };

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.block_on(fut)
        } else {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(fut)
        }
    }
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
