use http::{HeaderMap, Method, StatusCode};
use bytes::Bytes;
use smr_protocol::ApiProtocol;

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

pub type ProxyResponse = (StatusCode, HeaderMap, Bytes);
