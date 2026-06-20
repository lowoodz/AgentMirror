use serde_json::Value;

use crate::models::RunRecord;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

impl TokenUsage {
    pub fn is_empty(self) -> bool {
        self.prompt_tokens == 0 && self.completion_tokens == 0 && self.total_tokens == 0
    }

    pub fn merge_into(self, run: &mut RunRecord) {
        if self.is_empty() {
            return;
        }
        run.prompt_tokens = run.prompt_tokens.saturating_add(self.prompt_tokens);
        run.completion_tokens = run.completion_tokens.saturating_add(self.completion_tokens);
        let delta = if self.total_tokens > 0 {
            self.total_tokens
        } else {
            self.prompt_tokens.saturating_add(self.completion_tokens)
        };
        run.total_tokens = run.total_tokens.saturating_add(delta);
    }
}

/// Extract token usage from an LLM proxy response body (JSON or SSE).
pub fn extract_token_usage(body: &[u8]) -> TokenUsage {
    if body.is_empty() {
        return TokenUsage::default();
    }
    if let Ok(json) = serde_json::from_slice::<Value>(body) {
        if let Some(usage) = usage_from_value(&json) {
            return usage;
        }
    }
    if looks_like_sse(body) {
        return usage_from_sse(body);
    }
    TokenUsage::default()
}

fn usage_from_value(json: &Value) -> Option<TokenUsage> {
    if let Some(usage) = json.get("usage").and_then(parse_usage_object) {
        return Some(usage);
    }
    json.get("usageMetadata")
        .and_then(parse_google_usage)
}

fn parse_usage_object(u: &Value) -> Option<TokenUsage> {
    let prompt = token_field(u, &["prompt_tokens", "input_tokens"]);
    let completion = token_field(u, &["completion_tokens", "output_tokens"]);
    let total = token_field(u, &["total_tokens"]);
    if prompt == 0 && completion == 0 && total == 0 {
        return None;
    }
    Some(TokenUsage {
        prompt_tokens: prompt,
        completion_tokens: completion,
        total_tokens: if total > 0 {
            total
        } else {
            prompt.saturating_add(completion)
        },
    })
}

fn parse_google_usage(u: &Value) -> Option<TokenUsage> {
    let prompt = token_field(u, &["promptTokenCount", "prompt_token_count"]);
    let completion = token_field(
        u,
        &[
            "candidatesTokenCount",
            "candidates_token_count",
            "completionTokenCount",
        ],
    );
    let total = token_field(u, &["totalTokenCount", "total_token_count"]);
    if prompt == 0 && completion == 0 && total == 0 {
        return None;
    }
    Some(TokenUsage {
        prompt_tokens: prompt,
        completion_tokens: completion,
        total_tokens: if total > 0 {
            total
        } else {
            prompt.saturating_add(completion)
        },
    })
}

fn token_field(obj: &Value, keys: &[&str]) -> u64 {
    for key in keys {
        if let Some(v) = obj.get(*key) {
            if let Some(n) = v.as_u64() {
                return n;
            }
            if let Some(n) = v.as_i64() {
                return n.max(0) as u64;
            }
        }
    }
    0
}

fn usage_from_sse(body: &[u8]) -> TokenUsage {
    let text = String::from_utf8_lossy(body);
    let mut last = TokenUsage::default();
    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("data:") {
            continue;
        }
        let payload = trimmed.strip_prefix("data:").unwrap_or("").trim();
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }
        let Ok(json) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        if let Some(usage) = json.get("usage").and_then(parse_usage_object) {
            last = usage;
        }
    }
    last
}

fn looks_like_sse(body: &[u8]) -> bool {
    let sample = body.iter().take(512).copied().collect::<Vec<_>>();
    let text = String::from_utf8_lossy(&sample);
    text.contains("data:") && (text.contains("choices") || text.contains("usage"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_json_usage() {
        let body = br#"{"choices":[{"message":{"content":"hi"}}],"usage":{"prompt_tokens":10,"completion_tokens":20,"total_tokens":30}}"#;
        let u = extract_token_usage(body);
        assert_eq!(u.prompt_tokens, 10);
        assert_eq!(u.completion_tokens, 20);
        assert_eq!(u.total_tokens, 30);
    }

    #[test]
    fn anthropic_json_usage() {
        let body = br#"{"content":[{"type":"text","text":"ok"}],"usage":{"input_tokens":5,"output_tokens":7}}"#;
        let u = extract_token_usage(body);
        assert_eq!(u.prompt_tokens, 5);
        assert_eq!(u.completion_tokens, 7);
        assert_eq!(u.total_tokens, 12);
    }

    #[test]
    fn sse_stream_usage() {
        let body = b"data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":4,\"total_tokens\":7}}\n\n";
        let u = extract_token_usage(body);
        assert_eq!(u.total_tokens, 7);
    }

    #[test]
    fn merge_into_run() {
        let mut run = RunRecord {
            run_id: "r".into(),
            agent_id: "a".into(),
            session_id: "s".into(),
            started_at: chrono::Utc::now(),
            ended_at: None,
            status: crate::models::RunStatus::Running,
            goal: String::new(),
            turn_count: 0,
            messages_seen: 0,
            graph_path: None,
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        };
        TokenUsage {
            prompt_tokens: 1,
            completion_tokens: 2,
            total_tokens: 3,
        }
        .merge_into(&mut run);
        assert_eq!(run.total_tokens, 3);
    }
}
