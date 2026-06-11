//! Buffer streaming SSE tool_call deltas until complete, then run operation/path security.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::ops::OperationSecurity;

#[derive(Default, Clone)]
struct AccumulatedCall {
    id: String,
    name: String,
    arguments: String,
}

/// Holds partial OpenAI-style `delta.tool_calls` until `finish_reason: tool_calls`.
#[derive(Default)]
pub struct SseToolCallGate {
    active: bool,
    model: Option<String>,
    buffered_lines: Vec<String>,
    calls: BTreeMap<usize, AccumulatedCall>,
}

pub enum GateLineOutcome {
    /// Forward this line unchanged (already formatted with trailing newline bytes).
    Forward(Vec<u8>),
    /// Line absorbed into the gate; emit nothing yet.
    Hold,
    /// Tool-call stream finished — emit returned lines instead of the held stream.
    Release(Vec<Vec<u8>>),
}

impl SseToolCallGate {
    pub fn ingest_line(
        &mut self,
        line: &[u8],
        ops: Option<&OperationSecurity>,
    ) -> GateLineOutcome {
        let line_str = std::str::from_utf8(line).unwrap_or("");
        let Some(data) = line_str.strip_prefix("data: ") else {
            return GateLineOutcome::Forward(line.to_vec());
        };
        let trimmed = data.trim();
        if trimmed == "[DONE]" {
            if self.active {
                self.buffered_lines.push(trimmed.to_string());
                return self.finalize(ops);
            }
            return GateLineOutcome::Forward(line.to_vec());
        }

        let Ok(json) = serde_json::from_str::<Value>(trimmed) else {
            return GateLineOutcome::Forward(line.to_vec());
        };

        if self.model.is_none() {
            if let Some(m) = json.get("model").and_then(|v| v.as_str()) {
                self.model = Some(m.to_string());
            }
        }

        let finish_tool_calls = json
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("finish_reason"))
            .and_then(|v| v.as_str())
            == Some("tool_calls");

        if let Some(delta) = json
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("delta"))
        {
            if delta.get("tool_calls").is_some() {
                self.active = true;
                self.merge_delta_tool_calls(delta);
                self.buffered_lines.push(trimmed.to_string());
                if finish_tool_calls {
                    return self.finalize(ops);
                }
                return GateLineOutcome::Hold;
            }
        }

        if self.active {
            self.buffered_lines.push(trimmed.to_string());
            if finish_tool_calls {
                return self.finalize(ops);
            }
            return GateLineOutcome::Hold;
        }

        GateLineOutcome::Forward(line.to_vec())
    }

    fn merge_delta_tool_calls(&mut self, delta: &Value) {
        let Some(items) = delta.get("tool_calls").and_then(|t| t.as_array()) else {
            return;
        };
        for tc in items {
            let index = tc.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let entry = self.calls.entry(index).or_default();
            if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
                if !id.is_empty() {
                    entry.id = id.to_string();
                }
            }
            if let Some(name) = tc
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|v| v.as_str())
            {
                if !name.is_empty() {
                    entry.name = name.to_string();
                }
            }
            if let Some(args) = tc
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(|v| v.as_str())
            {
                entry.arguments.push_str(args);
            }
        }
    }

    fn finalize(&mut self, ops: Option<&OperationSecurity>) -> GateLineOutcome {
        let model = self.model.clone().unwrap_or_else(|| "unknown".into());
        let calls: Vec<_> = self.calls.values().cloned().collect();
        let buffered = std::mem::take(&mut self.buffered_lines);
        self.active = false;
        self.calls.clear();

        if let Some(ops) = ops {
            for call in &calls {
                if !call.arguments.is_empty() {
                    if let Some(blocked) = ops.enforce_tool_call(&call.arguments) {
                        return GateLineOutcome::Release(emit_blocked_tool_stream(
                            call, &blocked, &model,
                        ));
                    }
                }
            }
        }

        GateLineOutcome::Release(replay_lines(&buffered))
    }

    fn reset(&mut self) {
        self.active = false;
        self.buffered_lines.clear();
        self.calls.clear();
    }
}

fn replay_lines(lines: &[String]) -> Vec<Vec<u8>> {
    lines
        .iter()
        .map(|l| {
            let mut out = b"data: ".to_vec();
            out.extend_from_slice(l.as_bytes());
            out.push(b'\n');
            out
        })
        .collect()
}

fn emit_blocked_tool_call_sse(call: &AccumulatedCall, blocked_args: &str, model: &str) -> Vec<Vec<u8>> {
    let id = if call.id.is_empty() {
        "smr-blocked".to_string()
    } else {
        call.id.clone()
    };
    let name = if call.name.is_empty() {
        "exec".to_string()
    } else {
        call.name.clone()
    };

    let chunk1 = json!({
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": blocked_args
                    }
                }]
            },
            "finish_reason": null,
            "index": 0
        }],
        "model": model,
        "object": "chat.completion.chunk"
    });
    let chunk2 = json!({
        "choices": [{
            "delta": {},
            "finish_reason": "tool_calls",
            "index": 0
        }],
        "model": model,
        "object": "chat.completion.chunk"
    });

    let mut out = Vec::new();
    for chunk in [chunk1, chunk2] {
        let mut line = b"data: ".to_vec();
        line.extend_from_slice(&serde_json::to_vec(&chunk).unwrap_or_default());
        line.push(b'\n');
        out.push(line);
    }
    out
}

fn emit_blocked_tool_stream(
    call: &AccumulatedCall,
    blocked_args: &str,
    model: &str,
) -> Vec<Vec<u8>> {
    emit_blocked_tool_call_sse(call, blocked_args, model)
}

/// Process a complete SSE body (buffered upstream response) with tool-call gating.
pub fn transform_buffered_sse_ops(body: &str, ops: Option<&OperationSecurity>) -> (String, u32) {
    let mut gate = SseToolCallGate::default();
    let mut out = String::new();
    let mut blocks = 0u32;

    for line in body.lines() {
        let line_bytes = line.as_bytes();
        match gate.ingest_line(line_bytes, ops) {
            GateLineOutcome::Forward(l) => {
                out.push_str(std::str::from_utf8(&l).unwrap_or(""));
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            GateLineOutcome::Hold => {}
            GateLineOutcome::Release(lines) => {
                if lines.len() <= 2 && lines.iter().any(|l| l.windows(11).any(|w| w == b"smr_blocked")) {
                    blocks += 1;
                }
                for l in lines {
                    out.push_str(std::str::from_utf8(&l).unwrap_or(""));
                }
            }
        }
    }
    (out, blocks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{OperationSecurityMode, PathProtectionLevel, PathProtectionRule};
    use std::path::PathBuf;

    fn sample_stat_stream() -> String {
        let mut lines = Vec::new();
        let chunks = [
            r#"{"choices":[{"delta":{"tool_calls":[{"function":{"arguments":"","name":"exec"},"id":"call_x","index":0,"type":"function"}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"function":{"arguments":"{\"command\":\""},"index":0}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"function":{"arguments":"stat /secure/vault/a.md"},"index":0}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"function":{"arguments":"\"}"},"index":0}]}}]}"#,
            r#"{"choices":[{"delta":{"content":""},"finish_reason":"tool_calls","index":0}]}"#,
        ];
        for c in chunks {
            lines.push(format!("data: {c}"));
        }
        lines.join("\n") + "\n"
    }

    #[test]
    fn gates_streaming_tool_call_until_finish_then_blocks_path() {
        let ops = OperationSecurity::new(
            &[],
            &[PathProtectionRule {
                id: "vault".into(),
                enabled: true,
                path: PathBuf::from("/secure/vault"),
                level: PathProtectionLevel::DenyAccess,
            }],
            OperationSecurityMode::Enforce,
            OperationSecurityMode::Enforce,
        )
        .unwrap();

        let (out, blocks) = transform_buffered_sse_ops(&sample_stat_stream(), Some(&ops));
        assert_eq!(blocks, 1);
        assert!(out.contains("SMR BLOCKED"));
        assert!(out.contains("路径防护"));
        assert!(!out.contains("stat /secure/vault"));
    }

    #[test]
    fn gates_streaming_tool_call_releases_safe_commands() {
        let ops = OperationSecurity::new(
            &[],
            &[PathProtectionRule {
                id: "vault".into(),
                enabled: true,
                path: PathBuf::from("/secure/vault"),
                level: PathProtectionLevel::DenyAccess,
            }],
            OperationSecurityMode::Enforce,
            OperationSecurityMode::Enforce,
        )
        .unwrap();

        let mut stream = sample_stat_stream();
        stream = stream.replace("/secure/vault", "/tmp/public");
        let (out, blocks) = transform_buffered_sse_ops(&stream, Some(&ops));
        assert_eq!(blocks, 0);
        assert!(out.contains("stat /tmp/public"));
    }
}
