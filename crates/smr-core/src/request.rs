use std::pin::Pin;

use futures::Stream;
use http::{HeaderMap, Method, StatusCode};
use bytes::Bytes;
use smr_protocol::ApiProtocol;

use crate::router::RouteBody;
use crate::sse_stream::{SsePassthroughStream, SseResponseTransformStream, SseTransformConfig};

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
            RouteBody::SseStream(stream) => ProxyBody::SseStream(Box::pin(stream)),
        }
    }

    pub fn wrap_sse_response(stream: SsePassthroughStream, config: SseTransformConfig) -> Self {
        let (prefix, rest) = stream.into_transform_parts();
        let inner = SsePassthroughStream::new(Bytes::new(), rest);
        let transform = SseResponseTransformStream::new(inner, config).with_prefix(prefix);
        ProxyBody::SseStream(Box::pin(transform))
    }
}

pub type ProxyResponse = (StatusCode, HeaderMap, ProxyBody);
