//! Internal unified request representation and provider adapters.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::convert::{anthropic_to_openai, openai_to_anthropic};
use crate::protocol::ApiProtocol;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedRequest {
    pub model: String,
    pub messages: Value,
    pub stream: bool,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
    pub system: Option<String>,
}

impl UnifiedRequest {
    pub fn from_openai(body: &Value) -> Self {
        Self {
            model: body
                .get("model")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string(),
            messages: body.get("messages").cloned().unwrap_or(Value::Array(vec![])),
            stream: body.get("stream").and_then(|s| s.as_bool()).unwrap_or(false),
            max_tokens: body.get("max_tokens").and_then(|m| m.as_u64()),
            temperature: body.get("temperature").and_then(|t| t.as_f64()),
            system: None,
        }
    }

    pub fn from_anthropic(body: &Value) -> Self {
        Self {
            model: body
                .get("model")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string(),
            messages: body.get("messages").cloned().unwrap_or(Value::Array(vec![])),
            stream: body.get("stream").and_then(|s| s.as_bool()).unwrap_or(false),
            max_tokens: body.get("max_tokens").and_then(|m| m.as_u64()),
            temperature: body.get("temperature").and_then(|t| t.as_f64()),
            system: body
                .get("system")
                .and_then(|s| s.as_str())
                .map(|s| s.to_string()),
        }
    }

    pub fn to_openai(&self) -> Value {
        let mut out = serde_json::json!({
            "model": self.model,
            "messages": self.messages,
            "stream": self.stream,
        });
        if let Some(t) = self.temperature {
            out["temperature"] = serde_json::json!(t);
        }
        if let Some(m) = self.max_tokens {
            out["max_tokens"] = serde_json::json!(m);
        }
        out
    }

    pub fn to_anthropic(&self) -> Value {
        let mut out = serde_json::json!({
            "model": self.model,
            "messages": self.messages,
            "max_tokens": self.max_tokens.unwrap_or(4096),
            "stream": self.stream,
        });
        if let Some(s) = &self.system {
            out["system"] = serde_json::json!(s);
        }
        if let Some(t) = self.temperature {
            out["temperature"] = serde_json::json!(t);
        }
        out
    }
}

pub trait ProviderAdapter {
    fn protocol(&self) -> ApiProtocol;
    fn encode_request(&self, unified: &UnifiedRequest) -> Value;
    fn decode_response(&self, body: &Value) -> Value;
}

pub struct OpenAiProvider;

impl ProviderAdapter for OpenAiProvider {
    fn protocol(&self) -> ApiProtocol {
        ApiProtocol::OpenAi
    }

    fn encode_request(&self, unified: &UnifiedRequest) -> Value {
        unified.to_openai()
    }

    fn decode_response(&self, body: &Value) -> Value {
        body.clone()
    }
}

pub struct AnthropicProvider;

impl ProviderAdapter for AnthropicProvider {
    fn protocol(&self) -> ApiProtocol {
        ApiProtocol::Anthropic
    }

    fn encode_request(&self, unified: &UnifiedRequest) -> Value {
        unified.to_anthropic()
    }

    fn decode_response(&self, body: &Value) -> Value {
        body.clone()
    }
}

pub fn provider_for(protocol: ApiProtocol) -> Box<dyn ProviderAdapter> {
    match protocol {
        ApiProtocol::OpenAi => Box::new(OpenAiProvider),
        ApiProtocol::Anthropic => Box::new(AnthropicProvider),
    }
}

pub fn body_to_unified(body: &Value, from: ApiProtocol) -> UnifiedRequest {
    match from {
        ApiProtocol::OpenAi => UnifiedRequest::from_openai(body),
        ApiProtocol::Anthropic => UnifiedRequest::from_anthropic(body),
    }
}

pub fn unified_to_body(unified: &UnifiedRequest, to: ApiProtocol) -> Value {
    match to {
        ApiProtocol::OpenAi => unified.to_openai(),
        ApiProtocol::Anthropic => unified.to_anthropic(),
    }
}

pub fn convert_body(body: &Value, from: ApiProtocol, to: ApiProtocol) -> Value {
    if from == to {
        return body.clone();
    }
    match (from, to) {
        (ApiProtocol::OpenAi, ApiProtocol::Anthropic) => openai_to_anthropic(body),
        (ApiProtocol::Anthropic, ApiProtocol::OpenAi) => anthropic_to_openai(body),
        _ => body.clone(),
    }
}
