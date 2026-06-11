use std::sync::Arc;

use anyhow::Result;
use bytes::Bytes;
use smr_protocol::{
    detect_protocol, extract_texts, filter_ops_request_fields, filter_tool_related, inject_response_texts, inject_texts,
    parse_json_body, serialize_json_body, ApiProtocol,
};
use tracing::info;
use uuid::Uuid;

use crate::audit::{protocol_label, RequestAudit};
use crate::config::TrafficRequestCapture;
use crate::events::EventKind;
use crate::provider;
use crate::proxy_path;
use crate::request::{ForwardRequest, ProxyBody, ProxyRequest, ProxyResponse};
use crate::router::{ForwardOptions, RouteBody, RouteResult};
use crate::sse_sanitize::sanitize_openai_client_json;
use crate::sse_stream::SseTransformConfig;
use crate::state::SharedApp;
use crate::streaming::{
    force_upstream_non_stream, is_sse_content_type, openai_chat_completion_to_sse,
    process_sse_response, request_has_tools, request_wants_stream,
};

pub struct ProxyService {
    app: Arc<SharedApp>,
}

impl ProxyService {
    pub fn new(app: Arc<SharedApp>) -> Self {
        Self { app }
    }

    pub async fn handle_api_request(&self, req: ProxyRequest<'_>) -> Result<ProxyResponse> {
        if provider::is_models_list_request(&req.method, req.path) {
            let snap = self.app.snapshot();
            let json = provider::openai_models_list(&snap.config);
            let body = Bytes::from(serde_json::to_vec(&json)?);
            return Ok((
                http::StatusCode::OK,
                {
                    let mut h = http::HeaderMap::new();
                    h.insert(
                        http::header::CONTENT_TYPE,
                        http::HeaderValue::from_static("application/json"),
                    );
                    h
                },
                ProxyBody::Buffered(body),
            ));
        }

        let snap = self.app.snapshot();
        let events = self.app.events.clone();
        let audit_id = Uuid::new_v4().to_string();
        let mut dlp_count = 0u32;
        let mut safety_blocks = 0u32;
        let mut safety_observations = 0u32;

        let ProxyRequest {
            session_id,
            fallback_group,
            path,
            headers,
            body,
            ..
        } = &req;

        let api_path = proxy_path::normalize_client_api_path(path);

        let model_from_body = provider::extract_model_from_body(body);
        let group_from_model = model_from_body
            .as_deref()
            .and_then(provider::resolve_group_from_model);
        let client_public_model = group_from_model.map(provider::public_model_id);
        let effective_fallback_group = fallback_group
            .map(|s| s.to_string())
            .or_else(|| group_from_model.map(|s| s.to_string()));

        let is_json = headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.contains("application/json"))
            .unwrap_or(false);

        let mut wants_stream = is_json && request_wants_stream(body);
        let client_wants_stream = wants_stream;
        let mut client_protocol = ApiProtocol::OpenAi;
        let mut forward_body = body.to_vec();
        let traffic_cfg = &snap.config.logging;
        let capture_request_before_dlp = traffic_cfg.save_traffic_bodies
            && traffic_cfg.traffic_request_capture == TrafficRequestCapture::BeforeDlp;

        if capture_request_before_dlp && is_json && !body.is_empty() {
            self.app.traffic.record(
                &audit_id,
                session_id,
                "request_in",
                body,
                traffic_cfg.traffic_max_body_bytes,
            );
        }

        if is_json && !body.is_empty() {
            let mut json = parse_json_body(body)?;
            client_protocol = detect_protocol(&api_path, headers, &json);
            let extracted = extract_texts(&json)?;
            let json_before_ops = if snap.config.pipeline.ops_active() {
                Some(json.clone())
            } else {
                None
            };

            if snap.config.pipeline.dlp_active() {
                snap.dlp.register_path_triggers(session_id, &json);
            }

            if snap.config.pipeline.ops_active() {
                let ops_fields = filter_ops_request_fields(&json, &extracted);
                let (ops_replacements, blocks, observes) =
                    snap.ops.process_fields_with_mode(&ops_fields)?;
                safety_blocks += blocks;
                safety_observations += observes;
                if !ops_replacements.is_empty() {
                    inject_texts(&mut json, &ops_replacements)?;
                    events.push(
                        EventKind::OpBlock,
                        format!("blocked {} dangerous request tool_call(s)", ops_replacements.len()),
                        None,
                    );
                }
            }

            let ops_modified = json_before_ops
                .as_ref()
                .is_some_and(|before| before != &json);

            if snap.config.pipeline.dlp_active() {
                let (dlp_replacements, count) = snap.dlp.process_request(
                    session_id,
                    &extracted,
                    &json,
                    ops_modified,
                )?;
                dlp_count += count as u32;
                if !dlp_replacements.is_empty() {
                    info!(count = dlp_replacements.len(), "DLP sanitized request fields");
                    inject_texts(&mut json, &dlp_replacements)?;
                    events.push(
                        EventKind::DlpReplace,
                        format!("sanitized {} field(s)", dlp_replacements.len()),
                        None,
                    );
                }
            }

            // Buffer upstream JSON for tool calls; re-emit as client SSE after sanitize/ops.
            if client_wants_stream && request_has_tools(&json) {
                force_upstream_non_stream(&mut json);
                wants_stream = false;
            }

            forward_body = serialize_json_body(&json)?;
        } else if is_json {
            let json = serde_json::json!({});
            client_protocol = detect_protocol(&api_path, headers, &json);
        }

        if traffic_cfg.save_traffic_bodies && is_json && !forward_body.is_empty() {
            if traffic_cfg.traffic_request_capture == TrafficRequestCapture::AfterDlp {
                self.app.traffic.record(
                    &audit_id,
                    session_id,
                    "request_in",
                    &forward_body,
                    traffic_cfg.traffic_max_body_bytes,
                );
            }
            self.app.traffic.record(
                &audit_id,
                session_id,
                "request_out",
                &forward_body,
                traffic_cfg.traffic_max_body_bytes,
            );
        }

        let (group_name, group) = snap
            .router
            .resolve_group(effective_fallback_group.as_deref())?;
        let forward = ForwardRequest {
            method: req.method.clone(),
            path: &api_path,
            query: req.query,
            headers: req.headers.clone(),
            body: Bytes::from(forward_body),
            protocol: client_protocol,
        };

        let RouteResult {
            attempt,
            fallback_chain,
            group_name: resolved_group,
        } = snap
            .router
            .forward_with_fallback(
                &group_name,
                &group,
                forward,
                ForwardOptions {
                    wants_stream,
                    client_protocol,
                },
            )
            .await?;

        let endpoint_protocol = attempt.endpoint.resolve_protocol();
        debug_assert_eq!(client_protocol, endpoint_protocol);
        let mut resp_headers = attempt.headers;

        let needs_stream_transform = wants_stream
            || snap.config.pipeline.dlp_active()
            || snap.config.pipeline.ops_active();

        let proxy_body = match attempt.body {
            RouteBody::SseStream(stream) => {
                if needs_stream_transform {
                    ProxyBody::wrap_sse_response(
                        stream,
                        SseTransformConfig {
                            session_id: session_id.to_string(),
                            dlp: if snap.config.pipeline.dlp_active() {
                                Some(snap.dlp.clone())
                            } else {
                                None
                            },
                            ops: if snap.config.pipeline.ops_active() {
                                Some(snap.ops.clone())
                            } else {
                                None
                            },
                            protocol: None,
                        },
                    )
                } else {
                    ProxyBody::SseStream(Box::pin(stream))
                }
            }
            RouteBody::Buffered(mut resp_body) => {
                if traffic_cfg.save_traffic_bodies && !resp_body.is_empty() {
                    self.app.traffic.record(
                        &audit_id,
                        session_id,
                        "response_in",
                        &resp_body,
                        traffic_cfg.traffic_max_body_bytes,
                    );
                }

                if (snap.config.pipeline.dlp_active() || snap.config.pipeline.ops_active())
                    && (is_sse_content_type(&resp_headers) || wants_stream)
                {
                    let before = resp_body.clone();
                    let (new_body, blocks, observes, sse_dlp) = process_sse_response(
                        &resp_body,
                        session_id,
                        if snap.config.pipeline.dlp_active() {
                            Some(&snap.dlp)
                        } else {
                            None
                        },
                        if snap.config.pipeline.ops_active() {
                            Some(&snap.ops)
                        } else {
                            None
                        },
                    )?;
                    resp_body = new_body;
                    safety_blocks += blocks;
                    safety_observations += observes;
                    dlp_count += sse_dlp;
                    if resp_body != before {
                        if sse_dlp > 0 {
                            events.push(
                                EventKind::DlpReplace,
                                format!("sanitized {} SSE response field(s)", sse_dlp),
                                None,
                            );
                        }
                        if blocks > 0 {
                            events.push(
                                EventKind::OpBlock,
                                "blocked dangerous tool_call in SSE stream",
                                None,
                            );
                        }
                    }
                } else if snap.config.pipeline.dlp_active() || snap.config.pipeline.ops_active() {
                    let resp_is_json = resp_headers
                        .get(http::header::CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok())
                        .map(|v| v.contains("application/json"))
                        .unwrap_or(false);

                    if resp_is_json && !resp_body.is_empty() && attempt.status.is_success() {
                        if let Ok(mut json) = parse_json_body(&resp_body) {
                            if snap.config.pipeline.dlp_active() {
                                let extracted = extract_texts(&json)?;
                                let (dlp_replacements, count) =
                                    snap.dlp.process_response(session_id, &json, &extracted)?;
                                dlp_count += count as u32;
                                if !dlp_replacements.is_empty() {
                                    inject_response_texts(&mut json, &dlp_replacements)?;
                                    resp_body = Bytes::from(serialize_json_body(&json)?);
                                    events.push(
                                        EventKind::DlpReplace,
                                        format!(
                                            "sanitized {} response field(s)",
                                            dlp_replacements.len()
                                        ),
                                        None,
                                    );
                                }
                            }

                            if snap.config.pipeline.ops_active() {
                                let extracted = extract_texts(&json)?;
                                let tool_only = filter_tool_related(&json, &extracted);
                                let (ops_replacements, blocks, observes) =
                                    snap.ops.process_fields_with_mode(&tool_only)?;
                                safety_blocks += blocks;
                                safety_observations += observes;
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
                }

                if client_protocol == ApiProtocol::OpenAi
                    && attempt.status.is_success()
                    && !resp_body.is_empty()
                {
                    let resp_is_json = resp_headers
                        .get(http::header::CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok())
                        .map(|v| v.contains("application/json"))
                        .unwrap_or(false);
                    if resp_is_json {
                        if let Ok(mut json) = parse_json_body(&resp_body) {
                            let _ = sanitize_openai_client_json(&mut json);
                            if client_wants_stream {
                                resp_body = openai_chat_completion_to_sse(&json)?;
                                resp_headers.insert(
                                    http::header::CONTENT_TYPE,
                                    http::HeaderValue::from_static("text/event-stream"),
                                );
                            } else {
                                resp_body = Bytes::from(serialize_json_body(&json)?);
                            }
                        }
                    }
                }

                if traffic_cfg.save_traffic_bodies && !resp_body.is_empty() {
                    self.app.traffic.record(
                        &audit_id,
                        session_id,
                        "response_out",
                        &resp_body,
                        traffic_cfg.traffic_max_body_bytes,
                    );
                }

                if let Some(ref public_model) = client_public_model {
                    provider::patch_response_model(&mut resp_body, public_model);
                }

                ProxyBody::Buffered(resp_body)
            }
        };

        let proxy_body = if traffic_cfg.save_traffic_bodies {
            match proxy_body {
                ProxyBody::SseStream(stream) => ProxyBody::SseStream(self.app.traffic.wrap_sse_stream(
                    stream,
                    &audit_id,
                    session_id,
                    "response_out",
                    traffic_cfg.traffic_max_body_bytes,
                )),
                other => other,
            }
        } else {
            proxy_body
        };

        let success = attempt.status.is_success();
        if success {
            events.push(
                EventKind::RouteSuccess,
                format!("routed to {}", attempt.endpoint.model),
                None,
            );
        } else if fallback_chain.len() > 1 {
            events.push(
                EventKind::RouteFallback,
                format!("fallback chain: {}", fallback_chain.join(" → ")),
                None,
            );
        }

        let audit_message = if success {
            format!("routed to {}", attempt.endpoint.model)
        } else if let ProxyBody::Buffered(ref b) = proxy_body {
            String::from_utf8_lossy(b).into_owned()
        } else {
            "streaming response".to_string()
        };

        let audit = RequestAudit {
            id: audit_id,
            timestamp: chrono::Utc::now(),
            session_id: session_id.to_string(),
            protocol: protocol_label(client_protocol).to_string(),
            fallback_group: resolved_group,
            fallback_chain,
            final_model: if success {
                Some(attempt.endpoint.model.clone())
            } else {
                None
            },
            dlp_replacements: dlp_count,
            safety_blocks,
            safety_observations,
            success,
            message: audit_message.clone(),
        };
        let _ = self.app.storage.insert_audit(&audit);
        events.push(EventKind::Info, audit.summary(), None);

        Ok((attempt.status, resp_headers, proxy_body))
    }
}
