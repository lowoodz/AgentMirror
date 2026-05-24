use std::sync::Arc;

use anyhow::Result;
use bytes::Bytes;
use smr_protocol::{
    detect_protocol, extract_texts, inject_response_texts, inject_texts, parse_json_body,
    serialize_json_body,
};
use tracing::info;

use crate::events::EventKind;
use crate::request::{ForwardRequest, ProxyRequest, ProxyResponse};
use crate::state::SharedApp;
use crate::streaming::{is_sse_content_type, process_sse_response, request_wants_stream};

pub struct ProxyService {
    app: Arc<SharedApp>,
}

impl ProxyService {
    pub fn new(app: Arc<SharedApp>) -> Self {
        Self { app }
    }

    pub async fn handle_api_request(&self, req: ProxyRequest<'_>) -> Result<ProxyResponse> {
        let snap = self.app.snapshot();
        let events = self.app.events.clone();

        let ProxyRequest {
            session_id,
            fallback_group,
            path,
            headers,
            body,
            ..
        } = &req;

        let is_json = headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.contains("application/json"))
            .unwrap_or(false);

        let wants_stream = is_json && request_wants_stream(body);

        let (forward_body, protocol) = if is_json && !body.is_empty() {
            let mut json = parse_json_body(body)?;
            let protocol = detect_protocol(path, headers, &json);

            let extracted = extract_texts(&json)?;
            let all_text = extracted
                .iter()
                .map(|e| e.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");

            let dlp_replacements = snap.dlp.process_request(session_id, &all_text, &extracted)?;
            if !dlp_replacements.is_empty() {
                info!(count = dlp_replacements.len(), "DLP sanitized request fields");
                inject_texts(&mut json, &dlp_replacements)?;
                events.push(
                    EventKind::DlpReplace,
                    format!("sanitized {} field(s)", dlp_replacements.len()),
                    None,
                );
            }

            (serialize_json_body(&json)?, protocol)
        } else {
            let json = serde_json::json!({});
            (body.to_vec(), detect_protocol(path, headers, &json))
        };

        let group = snap.router.resolve_group(*fallback_group)?;
        let forward = ForwardRequest {
            method: req.method.clone(),
            path: req.path,
            query: req.query,
            headers: req.headers.clone(),
            body: Bytes::from(forward_body),
            protocol,
        };

        let attempt = snap.router.forward_with_fallback(group, forward).await?;
        let mut resp_body = attempt.body;
        let resp_headers = attempt.headers.clone();

        if is_sse_content_type(&resp_headers) || wants_stream {
            let before = resp_body.clone();
            resp_body = process_sse_response(&resp_body, &snap.ops)?;
            if resp_body != before {
                events.push(
                    EventKind::OpBlock,
                    "blocked dangerous tool_call in SSE stream",
                    None,
                );
            }
        } else {
            let resp_is_json = resp_headers
                .get(http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.contains("application/json"))
                .unwrap_or(false);

            if resp_is_json && !resp_body.is_empty() && attempt.status.is_success() {
                if let Ok(mut json) = parse_json_body(&resp_body) {
                    let extracted = extract_texts(&json)?;
                    let ops_replacements = snap.ops.process_response(&extracted)?;
                    if !ops_replacements.is_empty() {
                        info!(
                            count = ops_replacements.len(),
                            "operation security blocked response fields"
                        );
                        inject_response_texts(&mut json, &ops_replacements)?;
                        resp_body = Bytes::from(serialize_json_body(&json)?);
                        events.push(
                            EventKind::OpBlock,
                            "blocked dangerous tool_call in response",
                            None,
                        );
                    }
                }
            }
        }

        if attempt.status.is_success() {
            events.push(
                EventKind::RouteSuccess,
                format!("routed to {}", attempt.endpoint.model),
                None,
            );
        }

        Ok((attempt.status, resp_headers, resp_body))
    }
}
