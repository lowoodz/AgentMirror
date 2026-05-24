pub mod body;
pub mod protocol;

pub use body::{
    extract_texts, inject_response_texts, inject_texts, parse_json_body, serialize_json_body,
    ExtractedText, TextPointer,
};
pub use protocol::{detect_protocol, ApiProtocol};
