//! Map tier prefixes in the proxy URL path to fallback group names.

/// Public URL path segment for the `low` fallback group.
pub const TIER_LITE: &str = "lite";

/// Canonical OpenAI-compatible chat path.
pub const PATH_CHAT_COMPLETIONS: &str = "/v1/chat/completions";

/// Canonical Anthropic messages path.
pub const PATH_MESSAGES: &str = "/v1/messages";

/// Canonical models list path.
pub const PATH_MODELS: &str = "/v1/models";

/// Resolve a tier URL segment to a fallback group id in config.
pub fn tier_segment_to_group(segment: &str) -> Option<&'static str> {
    match segment {
        "high" => Some("high"),
        "medium" => Some("medium"),
        "lite" | "low" => Some("low"),
        _ => None,
    }
}

fn collapse_slashes(path: &str) -> String {
    let mut out = path.to_string();
    while out.contains("//") {
        out = out.replace("//", "/");
    }
    out
}

fn strip_leading_v1(path: &str) -> String {
    let mut p = path.trim().to_string();
    if !p.starts_with('/') {
        p = format!("/{p}");
    }
    p = collapse_slashes(&p);
    while p.starts_with("/v1/v1/") {
        p = p.replacen("/v1", "", 1);
    }
    if p == "/v1" {
        return "/".to_string();
    }
    if let Some(rest) = p.strip_prefix("/v1/") {
        return format!("/{rest}");
    }
    p
}

/// Normalize client paths so one base URL (`http://host/v1`) works for OpenAI and Anthropic SDKs.
pub fn normalize_client_api_path(path: &str) -> String {
    let stripped = strip_leading_v1(path);
    let bare = stripped.trim_start_matches('/').to_ascii_lowercase();
    match bare.as_str() {
        "" | "v1" => PATH_CHAT_COMPLETIONS.to_string(),
        "chat/completions" | "completions" | "chat/completions/" => {
            PATH_CHAT_COMPLETIONS.to_string()
        }
        "messages" | "messages/" => PATH_MESSAGES.to_string(),
        "models" | "models/" => PATH_MODELS.to_string(),
        s if s.starts_with("chat/completions") => PATH_CHAT_COMPLETIONS.to_string(),
        s if s.starts_with("messages") => PATH_MESSAGES.to_string(),
        s if s.starts_with("models") => PATH_MODELS.to_string(),
        _ => {
            if stripped.contains("/messages") && !stripped.contains("chat/completions") {
                PATH_MESSAGES.to_string()
            } else if stripped.contains("chat/completions") || stripped.contains("/completions") {
                PATH_CHAT_COMPLETIONS.to_string()
            } else if stripped.contains("/models") {
                PATH_MODELS.to_string()
            } else if stripped.starts_with('/') {
                stripped
            } else {
                format!("/{stripped}")
            }
        }
    }
}

/// Split `/high/v1/chat/completions` → (`Some("high")`, `/v1/chat/completions`).
/// Plain `/v1/...` or alias `/messages` keeps tier unset and normalizes the API path.
pub fn split_tier_path(path: &str) -> (Option<&'static str>, String) {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return (None, "/".to_string());
    }
    let segments: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return (None, "/".to_string());
    }
    let Some(group) = tier_segment_to_group(segments[0]) else {
        return (None, normalize_client_api_path(trimmed));
    };
    let forward = if segments.len() > 1 {
        format!("/{}", segments[1..].join("/"))
    } else {
        "/".to_string()
    };
    (Some(group), normalize_client_api_path(&forward))
}

pub fn tier_proxy_url(listen: &str, _tier: &str) -> String {
    format!("http://{listen}/v1")
}

pub fn proxy_tier_urls(listen: &str) -> (String, String, String) {
    let base = tier_proxy_url(listen, "high");
    (base.clone(), base.clone(), base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_tier_strips_prefix() {
        let (g, p) = split_tier_path("/high/v1/chat/completions");
        assert_eq!(g, Some("high"));
        assert_eq!(p, PATH_CHAT_COMPLETIONS);
    }

    #[test]
    fn high_tier_accepts_alias_path() {
        let (g, p) = split_tier_path("/high/messages");
        assert_eq!(g, Some("high"));
        assert_eq!(p, PATH_MESSAGES);
    }

    #[test]
    fn lite_maps_to_low_group() {
        let (g, p) = split_tier_path("/lite/v1/messages");
        assert_eq!(g, Some("low"));
        assert_eq!(p, PATH_MESSAGES);
    }

    #[test]
    fn plain_v1_unchanged() {
        let (g, p) = split_tier_path("/v1/chat/completions");
        assert_eq!(g, None);
        assert_eq!(p, PATH_CHAT_COMPLETIONS);
    }

    #[test]
    fn normalizes_messages_alias() {
        assert_eq!(normalize_client_api_path("/messages"), PATH_MESSAGES);
        assert_eq!(normalize_client_api_path("/v1/messages"), PATH_MESSAGES);
        assert_eq!(normalize_client_api_path("/v1/v1/messages"), PATH_MESSAGES);
    }

    #[test]
    fn normalizes_chat_alias() {
        assert_eq!(normalize_client_api_path("/chat/completions"), PATH_CHAT_COMPLETIONS);
    }

    #[test]
    fn normalizes_models_alias() {
        assert_eq!(normalize_client_api_path("/models"), PATH_MODELS);
    }

    #[test]
    fn universal_base_url_is_v1_root() {
        assert_eq!(tier_proxy_url("127.0.0.1:8080", "high"), "http://127.0.0.1:8080/v1");
    }
}
