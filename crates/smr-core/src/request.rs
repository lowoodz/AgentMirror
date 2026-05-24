use std::pin::Pin;

use futures::Stream;
use http::{HeaderMap, Method, StatusCode};
use bytes::Bytes;
use smr_protocol::ApiProtocol;

use crate::router::RouteBody;
use crate::sse_stream::{SseOpsTransformStream, SsePassthroughStream};

pub struct ProxyRequest<'a> {
    pub session_id: &'a str,
    pub fallback_group: Option<&'a str>,
    pub method: Method,
    pub path: &'a str,
    pub query: Option<&'a str>,
    pub headers: HeaderMap,
    pub body: Bytes,
}

pub struct ForwardRequest<'a> {
    pub method: Method,
    pub path: &'a str,
    pub query: Option<&'a str>,
    pub headers: HeaderMap,
    pub body: Bytes,
    pub protocol: ApiProtocol,
}

pub enum ProxyBody {
    Buffered(Bytes),
    SseStream(
        Pin<Box<dyn Stream<Item = Result<Bytes, std::convert::Infallible>> + Send>>,
    ),
}

impl ProxyBody {
    pub fn from_route(body: RouteBody) -> Self {
        match body {
            RouteBody::Buffered(b) => ProxyBody::Buffered(b),
            RouteBody::SseStream(stream) => {
                ProxyBody::SseStream(Box::pin(stream))
            }
        }
    }

    pub fn wrap_sse_ops(
        stream: SsePassthroughStream,
        ops: std::sync::Arc<crate::ops::OperationSecurity>,
        mode: crate::config::OperationSecurityMode,
    ) -> Self {
        ProxyBody::SseStream(Box::pin(SseOpsTransformStream::new(stream, ops, mode)))
    }
}

pub type ProxyResponse = (StatusCode, HeaderMap, ProxyBody);
