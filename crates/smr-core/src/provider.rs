//! Virtual provider: one universal API base URL, fallback groups exposed as public model IDs.

use http::Method;
use serde::Serialize;

use crate::config::AppConfig;
use crate::proxy_path::{self, PATH_MODELS};

pub const PROVIDER_ID: &str = "saferoute";
pub const PROVIDER_NAME: &str = "LLM-SafeRoute";

/// Fallback groups surfaced as public models, in display order.
pub const TIER_GROUPS: [&str; 3] = ["high", "medium", "low"];

#[derive(Debug, Clone, Serialize)]
pub struct PublicModelInfo {
    pub id: String,
    pub group: String,
    pub label: String,
    pub tier: String,
}

pub fn public_model_id(group: &str) -> String {
    match group {
        "high" => "saferoute-high".to_string(),
        "medium" => "saferoute-medium".to_string(),
        "low" => "saferoute-lite".to_string(),
        other => format!("{PROVIDER_ID}-{other}"),
    }
}

pub fn public_model_label(group: &str) -> String {
    match group {
        "high" => "High precision".into(),
        "medium" => "Balanced".into(),
        "low" => "Lightweight".into(),
        other => other.to_string(),
    }
}

/// URL path segment shown in legacy tier routing (low group → `lite`).
pub fn tier_path_segment(group: &str) -> String {
    match group {
        "low" => "lite".into(),
        _ => group.to_string(),
    }
}

/// Map a client `model` field to an internal fallback group id.
pub fn resolve_group_from_model(model: &str) -> Option<&'static str> {
    let m = model.trim();
    if m.is_empty() {
        return None;
    }
    let lower = m.to_ascii_lowercase();
    let normalized = lower
        .strip_prefix("saferoute/")
        .or_else(|| lower.strip_prefix("saferoute:"))
        .unwrap_or(&lower);
    match normalized {
        "high" | "saferoute-high" => Some("high"),
        "medium" | "saferoute-medium" => Some("medium"),
        "low" | "lite" | "saferoute-lite" | "saferoute-low" => Some("low"),
        _ if normalized.starts_with("saferoute-") => {
            let suffix = normalized.trim_start_matches("saferoute-");
            match suffix {
                "high" => Some("high"),
                "medium" => Some("medium"),
                "lite" | "low" => Some("low"),
                _ => None,
            }
        }
        _ => None,
    }
}

pub fn is_public_model(model: &str) -> bool {
    resolve_group_from_model(model).is_some()
}

pub fn list_public_models(config: &AppConfig) -> Vec<PublicModelInfo> {
    TIER_GROUPS
        .iter()
        .copied()
        .filter(|g| config.fallback_groups.contains_key(*g))
        .map(|g| PublicModelInfo {
            id: public_model_id(g),
            group: g.to_string(),
            label: public_model_label(g),
            tier: tier_path_segment(g),
        })
        .collect()
}

pub fn openai_models_list(config: &AppConfig) -> serde_json::Value {
    let now = chrono::Utc::now().timestamp();
    let data: Vec<serde_json::Value> = list_public_models(config)
        .into_iter()
        .map(|m| {
            serde_json::json!({
                "id": m.id,
                "object": "model",
                "created": now,
                "owned_by": PROVIDER_ID,
            })
        })
        .collect();
    serde_json::json!({
        "object": "list",
        "data": data,
    })
}

pub fn is_models_list_request(method: &Method, path: &str) -> bool {
    if *method != Method::GET {
        return false;
    }
    proxy_path::normalize_client_api_path(path) == PATH_MODELS
}

pub fn extract_model_from_body(body: &[u8]) -> Option<String> {
    if body.is_empty() {
        return None;
    }
    let json: serde_json::Value = serde_json::from_slice(body).ok()?;
    json.get("model")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Universal API base URL: OpenAI (`/chat/completions`) and Anthropic (`/messages`) share this root.
pub fn provider_base_url(listen: &str) -> String {
    format!("http://{listen}/v1")
}

pub fn patch_response_model(body: &mut bytes::Bytes, client_model: &str) -> bool {
    if body.is_empty() || client_model.is_empty() {
        return false;
    }
    let Ok(mut json) = serde_json::from_slice::<serde_json::Value>(body) else {
        return false;
    };
    if json.get("model").is_none() {
        return false;
    }
    json["model"] = serde_json::Value::String(client_model.to_string());
    if let Ok(bytes) = serde_json::to_vec(&json) {
        *body = bytes::Bytes::from(bytes);
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn sample_config() -> AppConfig {
        let mut groups = HashMap::new();
        groups.insert("high".into(), vec![]);
        groups.insert("medium".into(), vec![]);
        groups.insert("low".into(), vec![]);
        AppConfig {
            server: Default::default(),
            pipeline: Default::default(),
            logging: Default::default(),
            fallback_groups: groups,
            content_rules: vec![],
            file_rules: vec![],
            operation_rules: vec![],
            path_protection_rules: vec![],
        }
    }

    #[test]
    fn maps_public_model_ids() {
        assert_eq!(resolve_group_from_model("saferoute-high"), Some("high"));
        assert_eq!(resolve_group_from_model("saferoute-medium"), Some("medium"));
        assert_eq!(resolve_group_from_model("saferoute-lite"), Some("low"));
        assert_eq!(resolve_group_from_model("saferoute/high"), Some("high"));
        assert_eq!(resolve_group_from_model("high"), Some("high"));
    }

    #[test]
    fn ignores_upstream_model_names() {
        assert_eq!(resolve_group_from_model("gpt-4o-mini"), None);
    }

    #[test]
    fn models_list_path() {
        assert!(is_models_list_request(&Method::GET, "/v1/models"));
        assert!(is_models_list_request(&Method::GET, "/models"));
        assert!(!is_models_list_request(&Method::POST, "/v1/models"));
    }

    #[test]
    fn lists_configured_groups() {
        let models = list_public_models(&sample_config());
        assert_eq!(models.len(), 3);
        assert_eq!(models[0].id, "saferoute-high");
    }
}
