use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures::Stream;
use http_body::Body as HttpBody;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use smr_protocol::{convert_sse_chunk, ApiProtocol};

use crate::router::sse_has_first_token;
use crate::sse_sanitize::sanitize_openai_client_sse_chunk;
use crate::sse_tool_ops::{GateLineOutcome, SseToolCallGate};

pub enum SseCollectResult {
    /// Stream ended with no first token (candidate for fallback).
    NoFirstToken(Bytes),
    /// Not treated as live SSE (buffered entirely).
    Buffered(Bytes),
    /// First token seen; prefix is already read, `rest` continues upstream.
    Passthrough { prefix: Bytes, rest: Incoming },
}

/// Read an upstream body until SSE first token or EOF.
pub async fn collect_sse_for_routing(mut body: Incoming) -> anyhow::Result<SseCollectResult> {
    let mut buf = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame?;
        if let Some(chunk) = frame.data_ref() {
            buf.extend_from_slice(chunk);
            if sse_has_first_token(&buf) {
                return Ok(SseCollectResult::Passthrough {
                    prefix: Bytes::from(buf),
                    rest: body,
                });
            }
        }
    }
    let bytes = Bytes::from(buf);
    if bytes.is_empty() {
        Ok(SseCollectResult::NoFirstToken(bytes))
    } else if sse_has_first_token(&bytes) {
        Ok(SseCollectResult::Buffered(bytes))
    } else {
        Ok(SseCollectResult::NoFirstToken(bytes))
    }
}

/// Stream that yields `prefix` once, then polls `Incoming`.
pub struct SsePassthroughStream {
    prefix: Option<Bytes>,
    inner: Incoming,
}

impl SsePassthroughStream {
    pub fn new(prefix: Bytes, rest: Incoming) -> Self {
        Self {
            prefix: Some(prefix),
            inner: rest,
        }
    }

    pub fn into_transform_parts(mut self) -> (Bytes, Incoming) {
        (self.prefix.take().unwrap_or_default(), self.inner)
    }
}

impl Stream for SsePassthroughStream {
    type Item = Result<Bytes, std::convert::Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(prefix) = self.prefix.take() {
            return Poll::Ready(Some(Ok(prefix)));
        }
        loop {
            match Pin::new(&mut self.inner).poll_frame(cx) {
                Poll::Ready(Some(Ok(frame))) => {
                    if let Some(data) = frame.data_ref() {
                        return Poll::Ready(Some(Ok(Bytes::copy_from_slice(data))));
                    }
                }
                Poll::Ready(Some(Err(e))) => {
                    tracing::warn!(error = %e, "upstream SSE stream error");
                    return Poll::Ready(None);
                }
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// Unified SSE line processor: protocol conversion, DLP, and operation security.
pub struct SseResponseTransformStream<S> {
    inner: S,
    line_buf: Vec<u8>,
    session_id: String,
    dlp: Option<std::sync::Arc<crate::dlp::DlpEngine>>,
    ops: Option<std::sync::Arc<crate::ops::OperationSecurity>>,
    protocol: Option<(ApiProtocol, ApiProtocol)>,
    tool_gate: SseToolCallGate,
    pending_out: Vec<Vec<u8>>,
}

impl<S> SseResponseTransformStream<S> {
    pub fn new(inner: S, config: SseTransformConfig) -> Self {
        Self {
            inner,
            line_buf: Vec::new(),
            session_id: config.session_id,
            dlp: config.dlp,
            ops: config.ops,
            protocol: config.protocol,
            tool_gate: SseToolCallGate::default(),
            pending_out: Vec::new(),
        }
    }

    /// Seed the line buffer with bytes already read for routing (passthrough prefix).
    pub fn with_prefix(mut self, prefix: Bytes) -> Self {
        if !prefix.is_empty() {
            self.line_buf.extend_from_slice(&prefix);
        }
        self
    }

    fn pop_pending(&mut self) -> Option<Vec<u8>> {
        self.pending_out.pop()
    }

    fn transform_forward_json(&self, mut json: serde_json::Value) -> Option<Vec<u8>> {
        if let Some((from, to)) = self.protocol {
            json = convert_sse_chunk(&json, from, to);
        }

        if !sanitize_openai_client_sse_chunk(&mut json) {
            return None;
        }

        if let Some(dlp) = &self.dlp {
            if let Ok(extracted) = smr_protocol::extract_texts(&json) {
                if let Ok((replacements, _)) =
                    dlp.process_response(&self.session_id, &json, &extracted)
                {
                    if !replacements.is_empty() {
                        let _ = smr_protocol::inject_response_texts(&mut json, &replacements);
                    }
                }
            }
        }

        if let Ok(bytes) = smr_protocol::serialize_json_body(&json) {
            let mut out = b"data: ".to_vec();
            out.extend_from_slice(&bytes);
            out.push(b'\n');
            return Some(out);
        }
        None
    }

    fn process_line(&mut self, line: &[u8]) -> Option<Vec<u8>> {
        let line_str = std::str::from_utf8(line).unwrap_or("");
        if line_str.strip_prefix("data: ").is_some() {
            if let Some(ops) = &self.ops {
                let outcome = self.tool_gate.ingest_line(line, Some(ops.as_ref()));
                return match outcome {
                    GateLineOutcome::Forward(l) => self.process_forward_line(&l),
                    GateLineOutcome::Hold => None,
                    GateLineOutcome::Release(mut lines) => {
                        self.pending_out.append(&mut lines);
                        self.pop_pending()
                    }
                };
            }
        }

        self.process_forward_line(line)
    }

    fn process_forward_line(&self, line: &[u8]) -> Option<Vec<u8>> {
        let line_str = std::str::from_utf8(line).unwrap_or("");
        if let Some(data) = line_str.strip_prefix("data: ") {
            let trimmed = data.trim();
            if trimmed == "[DONE]" {
                let mut out = line.to_vec();
                out.push(b'\n');
                return Some(out);
            }
            if let Ok(json) = smr_protocol::parse_json_body(trimmed.as_bytes()) {
                return self.transform_forward_json(json);
            }
        }
        let mut out = line.to_vec();
        out.push(b'\n');
        Some(out)
    }
}

pub struct SseTransformConfig {
    pub session_id: String,
    pub dlp: Option<std::sync::Arc<crate::dlp::DlpEngine>>,
    pub ops: Option<std::sync::Arc<crate::ops::OperationSecurity>>,
    pub protocol: Option<(ApiProtocol, ApiProtocol)>,
}

impl<S: Stream<Item = Result<Bytes, std::convert::Infallible>> + Unpin> Stream
    for SseResponseTransformStream<S>
{
    type Item = Result<Bytes, std::convert::Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(out) = self.pop_pending() {
                return Poll::Ready(Some(Ok(Bytes::from(out))));
            }

            if let Some(pos) = self.line_buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.line_buf.drain(..=pos).collect();
                let line = &line[..line.len().saturating_sub(1)];
                if let Some(out) = self.process_line(line) {
                    return Poll::Ready(Some(Ok(Bytes::from(out))));
                }
                continue;
            }

            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    self.line_buf.extend_from_slice(&chunk);
                }
                Poll::Ready(other) => {
                    let this = self.as_mut().get_mut();
                    if !this.line_buf.is_empty() {
                        let tail = std::mem::take(&mut this.line_buf);
                        if let Some(out) = this.process_line(&tail) {
                            return Poll::Ready(Some(Ok(Bytes::from(out))));
                        }
                    }
                    if this.ops.is_some() {
                        let ops_ref = this.ops.as_ref().map(|o| o.as_ref());
                        let outcome = this.tool_gate.flush_eof(ops_ref);
                        if let GateLineOutcome::Release(mut lines) = outcome {
                            this.pending_out.append(&mut lines);
                            if let Some(out) = this.pending_out.pop() {
                                return Poll::Ready(Some(Ok(Bytes::from(out))));
                            }
                        }
                    }
                    return Poll::Ready(other);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
