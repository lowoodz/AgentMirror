use std::sync::Arc;

use anyhow::Result;
use bytes::Bytes;
use smr_protocol::{
    detect_protocol, extract_texts, filter_tool_related, inject_response_texts, inject_texts,
    parse_json_body, serialize_json_body, ApiProtocol,
};
use tracing::info;
use uuid::Uuid;

use crate::audit::{protocol_label, RequestAudit};
use crate::events::EventKind;
use crate::request::{ForwardRequest, ProxyBody, ProxyRequest, ProxyResponse};
use crate::router::{convert_response_body, ForwardOptions, RouteBody, RouteResult};
use crate::sse_stream::SseTransformConfig;
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

        let is_json = headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.contains("application/json"))
            .unwrap_or(false);

        let wants_stream = is_json && request_wants_stream(body);
        let mut client_protocol = ApiProtocol::OpenAi;
        let mut forward_body = body.to_vec();
        let traffic_cfg = &snap.config.logging;

        if traffic_cfg.save_traffic_bodies && is_json && !body.is_empty() {
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
            client_protocol = detect_protocol(path, headers, &json);
            let extracted = extract_texts(&json)?;

            if snap.config.pipeline.dlp_active() {
                snap.dlp.register_path_triggers(session_id, &json);
            }

            if snap.config.pipeline.ops_active() {
                let tool_only = filter_tool_related(&json, &extracted);
                let (ops_replacements, blocks, observes) =
                    snap.ops.process_fields_with_mode(&tool_only)?;
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

            if snap.config.pipeline.dlp_active() {
                let (dlp_replacements, count) =
                    snap.dlp.process_request(session_id, &extracted, &json)?;
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

            forward_body = serialize_json_body(&json)?;
        } else if is_json {
            let json = serde_json::json!({});
            client_protocol = detect_protocol(path, headers, &json);
        }

        if traffic_cfg.save_traffic_bodies && is_json && !forward_body.is_empty() {
            self.app.traffic.record(
                &audit_id,
                session_id,
                "request_out",
                &forward_body,
                traffic_cfg.traffic_max_body_bytes,
            );
        }

        let (group_name, group) = snap.router.resolve_group(*fallback_group)?;
        let forward = ForwardRequest {
            method: req.method.clone(),
            path: req.path,
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
        let resp_headers = attempt.headers;

        let needs_stream_transform = snap.config.pipeline.dlp_active()
            || snap.config.pipeline.ops_active()
            || (client_protocol != endpoint_protocol);

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
                                Some((
                                    snap.ops.clone(),
                                    snap.config.pipeline.operation_security_mode,
                                ))
                            } else {
                                None
                            },
                            protocol: if client_protocol != endpoint_protocol {
                                Some((endpoint_protocol, client_protocol))
                            } else {
                                None
                            },
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

                if client_protocol != endpoint_protocol
                    && !resp_body.is_empty()
                    && attempt.status.is_success()
                    && !is_sse_content_type(&resp_headers)
                {
                    if let Ok(converted) =
                        convert_response_body(&resp_body, endpoint_protocol, client_protocol)
                    {
                        resp_body = converted;
                    }
                }

                if snap.config.pipeline.ops_active()
                    && (is_sse_content_type(&resp_headers) || wants_stream)
                {
                    let before = resp_body.clone();
                    let (new_body, blocks, observes) = process_sse_response(
                        &resp_body,
                        &snap.ops,
                        snap.config.pipeline.operation_security_mode,
                    )?;
                    resp_body = new_body;
                    safety_blocks += blocks;
                    safety_observations += observes;
                    if resp_body != before {
                        events.push(
                            EventKind::OpBlock,
                            "blocked dangerous tool_call in SSE stream",
                            None,
                        );
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

                if traffic_cfg.save_traffic_bodies && !resp_body.is_empty() {
                    self.app.traffic.record(
                        &audit_id,
                        session_id,
                        "response_out",
                        &resp_body,
                        traffic_cfg.traffic_max_body_bytes,
                    );
                }

                ProxyBody::Buffered(resp_body)
            }
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
