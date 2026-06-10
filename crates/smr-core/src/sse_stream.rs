use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures::Stream;
use http_body::Body as HttpBody;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use smr_protocol::{convert_sse_chunk, ApiProtocol};

use crate::router::sse_has_first_token;

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
        }
    }

    fn process_line(&self, line: &[u8]) -> Vec<u8> {
        let line_str = String::from_utf8_lossy(line);
        if let Some(data) = line_str.strip_prefix("data: ") {
            let trimmed = data.trim();
            if trimmed == "[DONE]" {
                let mut out = line.to_vec();
                out.push(b'\n');
                return out;
            }
            if let Ok(mut json) = smr_protocol::parse_json_body(trimmed.as_bytes()) {
                if let Some((from, to)) = self.protocol {
                    json = convert_sse_chunk(&json, from, to);
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

                if let Some(ops) = &self.ops {
                    if let Ok(extracted) = smr_protocol::extract_texts(&json) {
                        let tool_only = smr_protocol::filter_tool_related(&json, &extracted);
                        if let Ok((replacements, _, _)) = ops.process_fields_with_mode(&tool_only)
                        {
                            if !replacements.is_empty() {
                                let _ =
                                    smr_protocol::inject_response_texts(&mut json, &replacements);
                            }
                        }
                    }
                }

                if let Ok(bytes) = smr_protocol::serialize_json_body(&json) {
                    let mut out = b"data: ".to_vec();
                    out.extend_from_slice(&bytes);
                    out.push(b'\n');
                    return out;
                }
            }
        }
        let mut out = line.to_vec();
        out.push(b'\n');
        out
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
            if let Some(pos) = self.line_buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.line_buf.drain(..=pos).collect();
                let line = &line[..line.len().saturating_sub(1)];
                let out = self.process_line(line);
                return Poll::Ready(Some(Ok(Bytes::from(out))));
            }

            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    self.line_buf.extend_from_slice(&chunk);
                }
                Poll::Ready(other) => {
                    if !self.line_buf.is_empty() {
                        let tail = std::mem::take(&mut self.line_buf);
                        let out = self.process_line(&tail);
                        return Poll::Ready(Some(Ok(Bytes::from(out))));
                    }
                    return Poll::Ready(other);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// Incrementally scan SSE lines and optionally rewrite `data:` JSON payloads.
pub struct SseOpsTransformStream<S> {
    inner: S,
    line_buf: Vec<u8>,
    ops: std::sync::Arc<crate::ops::OperationSecurity>,
    blocks: std::sync::atomic::AtomicU32,
    observes: std::sync::atomic::AtomicU32,
}

impl<S> SseOpsTransformStream<S> {
    pub fn new(inner: S, ops: std::sync::Arc<crate::ops::OperationSecurity>) -> Self {
        Self {
            inner,
            line_buf: Vec::new(),
            ops,
            blocks: std::sync::atomic::AtomicU32::new(0),
            observes: std::sync::atomic::AtomicU32::new(0),
        }
    }

    pub fn counters(&self) -> (u32, u32) {
        (
            self.blocks.load(std::sync::atomic::Ordering::Relaxed),
            self.observes.load(std::sync::atomic::Ordering::Relaxed),
        )
    }

    fn process_line(&self, line: &[u8]) -> Vec<u8> {
        let line_str = String::from_utf8_lossy(line);
        if let Some(data) = line_str.strip_prefix("data: ") {
            let trimmed = data.trim();
            if trimmed == "[DONE]" {
                return line.to_vec();
            }
            if let Ok(mut json) = smr_protocol::parse_json_body(trimmed.as_bytes()) {
                if let Ok(extracted) = smr_protocol::extract_texts(&json) {
                    if let Ok((replacements, b, o)) =
                        self.ops.process_fields_with_mode(&extracted)
                    {
                        self.blocks
                            .fetch_add(b, std::sync::atomic::Ordering::Relaxed);
                        self.observes
                            .fetch_add(o, std::sync::atomic::Ordering::Relaxed);
                        if !replacements.is_empty() {
                            if smr_protocol::inject_response_texts(&mut json, &replacements).is_ok()
                            {
                                if let Ok(bytes) = smr_protocol::serialize_json_body(&json) {
                                    let mut out = b"data: ".to_vec();
                                    out.extend_from_slice(&bytes);
                                    out.push(b'\n');
                                    return out;
                                }
                            }
                        }
                    }
                }
            }
        }
        let mut out = line.to_vec();
        out.push(b'\n');
        out
    }
}

impl<S: Stream<Item = Result<Bytes, std::convert::Infallible>> + Unpin> Stream
    for SseOpsTransformStream<S>
{
    type Item = Result<Bytes, std::convert::Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(pos) = self.line_buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.line_buf.drain(..=pos).collect();
                let line = &line[..line.len().saturating_sub(1)];
                let out = self.process_line(line);
                return Poll::Ready(Some(Ok(Bytes::from(out))));
            }

            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    self.line_buf.extend_from_slice(&chunk);
                }
                Poll::Ready(other) => {
                    if !self.line_buf.is_empty() {
                        let tail = std::mem::take(&mut self.line_buf);
                        let out = self.process_line(&tail);
                        return Poll::Ready(Some(Ok(Bytes::from(out))));
                    }
                    return Poll::Ready(other);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
