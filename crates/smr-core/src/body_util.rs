use futures::StreamExt;
use http_body::Frame;
use http_body_util::StreamBody;

use crate::request::ProxyBody;

pub fn proxy_body_to_axum(body: ProxyBody) -> axum::body::Body {
    match body {
        ProxyBody::Buffered(bytes) => axum::body::Body::from(bytes),
        ProxyBody::SseStream(stream) => {
            let mapped = stream.map(|result| result.map(Frame::data));
            axum::body::Body::new(StreamBody::new(mapped))
        }
    }
}

pub fn apply_streaming_headers(headers: &mut axum::http::HeaderMap) {
    headers.remove(axum::http::header::CONTENT_LENGTH);
    headers.insert(
        axum::http::header::TRANSFER_ENCODING,
        axum::http::header::HeaderValue::from_static("chunked"),
    );
}
