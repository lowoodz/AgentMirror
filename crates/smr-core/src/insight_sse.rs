use std::collections::BTreeMap;
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use bytes::Bytes;
use futures::Stream;
use serde_json::{json, Value};
use smr_insight::{InsightService, TraceTurn};

/// Insight SSE tap keeps at least 8 MiB even when traffic snapshots use a smaller cap.
const INSIGHT_SSE_MIN_BYTES: usize = 8 * 1024 * 1024;
/// Logical aggregated assistant/tool payload budget (independent of raw SSE capture).
const INSIGHT_AGG_MAX_BYTES: usize = 2 * 1024 * 1024;

pub fn insight_sse_byte_limit(traffic_max: usize) -> usize {
    traffic_max.max(INSIGHT_SSE_MIN_BYTES)
}

#[derive(Default, Clone)]
struct AggregatedToolCall {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Default)]
struct SseInsightAggregator {
    content: String,
    tool_calls: BTreeMap<usize, AggregatedToolCall>,
    args_prepend_order: Option<bool>,
    finish_reason: Option<String>,
}

impl SseInsightAggregator {
    fn ingest_line(&mut self, line: &str) {
        let trimmed = line.trim();
        if !trimmed.starts_with("data:") {
            return;
        }
        let payload = trimmed.strip_prefix("data:").unwrap_or("").trim();
        if payload.is_empty() || payload == "[DONE]" {
            return;
        }
        let Ok(json) = serde_json::from_str::<Value>(payload) else {
            return;
        };
        let choice = json.get("choices").and_then(|c| c.as_array()).and_then(|a| a.first());
        if let Some(reason) = choice
            .and_then(|c| c.get("finish_reason"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            self.finish_reason = Some(reason.to_string());
        }
        if let Some(delta) = choice.and_then(|c| c.get("delta")) {
            if let Some(text) = delta.get("content").and_then(|c| c.as_str()) {
                if self.content.len() < INSIGHT_AGG_MAX_BYTES {
                    let room = INSIGHT_AGG_MAX_BYTES - self.content.len();
                    self.content.push_str(&text[..text.len().min(room)]);
                }
            }
            if let Some(items) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                for tc in items {
                    let index = tc.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                    let entry = self.tool_calls.entry(index).or_default();
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
                        merge_argument_fragment(entry, args, &mut self.args_prepend_order);
                    }
                }
            }
        }
    }

    fn has_payload(&self) -> bool {
        !self.content.is_empty() || !self.tool_calls.is_empty()
    }

    fn to_response_json(&self) -> Vec<u8> {
        let mut tool_calls = Vec::new();
        for (idx, call) in &self.tool_calls {
            if call.name.is_empty() && call.arguments.is_empty() {
                continue;
            }
            tool_calls.push(json!({
                "id": if call.id.is_empty() { format!("call_{idx}") } else { call.id.clone() },
                "type": "function",
                "function": {
                    "name": call.name,
                    "arguments": call.arguments,
                }
            }));
        }
        let finish = self
            .finish_reason
            .clone()
            .unwrap_or_else(|| if tool_calls.is_empty() { "stop".into() } else { "tool_calls".into() });
        let message = if tool_calls.is_empty() {
            json!({
                "role": "assistant",
                "content": self.content,
            })
        } else {
            json!({
                "role": "assistant",
                "content": if self.content.is_empty() { Value::Null } else { Value::String(self.content.clone()) },
                "tool_calls": tool_calls,
            })
        };
        let body = json!({
            "choices": [{
                "message": message,
                "finish_reason": finish,
            }]
        });
        serde_json::to_vec(&body).unwrap_or_default()
    }
}

fn merge_argument_fragment(
    entry: &mut AggregatedToolCall,
    fragment: &str,
    prepend_order: &mut Option<bool>,
) {
    if fragment.is_empty() {
        return;
    }
    match prepend_order {
        Some(true) => {
            entry.arguments = format!("{fragment}{}", entry.arguments);
        }
        Some(false) => entry.arguments.push_str(fragment),
        None => {
            if entry.arguments.is_empty() {
                entry.arguments.push_str(fragment);
                *prepend_order = Some(!fragment.starts_with('{'));
            } else if fragment.starts_with('{') || entry.arguments.starts_with('{') {
                entry.arguments.push_str(fragment);
                *prepend_order = Some(false);
            } else {
                entry.arguments = format!("{fragment}{}", entry.arguments);
                *prepend_order = Some(true);
            }
        }
    }
    if entry.arguments.len() > INSIGHT_AGG_MAX_BYTES {
        entry.arguments.truncate(INSIGHT_AGG_MAX_BYTES);
    }
}

struct InsightTapState {
    raw: Mutex<Vec<u8>>,
    raw_max: usize,
    line_buf: Mutex<String>,
    aggregator: Mutex<SseInsightAggregator>,
    recorded: AtomicBool,
    turn: TraceTurn,
    insight: Arc<InsightService>,
}

impl InsightTapState {
    fn push(&self, chunk: &[u8]) {
        {
            let mut raw = self.raw.lock().unwrap();
            if raw.len() < self.raw_max {
                let take = (self.raw_max - raw.len()).min(chunk.len());
                raw.extend_from_slice(&chunk[..take]);
            }
        }
        let mut pending = self.line_buf.lock().unwrap();
        pending.push_str(&String::from_utf8_lossy(chunk));
        let mut complete: Vec<String> = Vec::new();
        while let Some(pos) = pending.find('\n') {
            let line = pending[..pos].to_string();
            pending.drain(..=pos);
            complete.push(line);
        }
        if !complete.is_empty() {
            let mut agg = self.aggregator.lock().unwrap();
            for line in complete {
                agg.ingest_line(&line);
            }
        }
    }

    fn flush(&self) {
        if self.recorded.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Ok(mut pending) = self.line_buf.lock() {
            if !pending.trim().is_empty() {
                let mut agg = self.aggregator.lock().unwrap();
                agg.ingest_line(pending.trim());
            }
            pending.clear();
        }
        let mut turn = self.turn.clone();
        let aggregated = {
            let agg = self.aggregator.lock().unwrap();
            if agg.has_payload() {
                Some(agg.to_response_json())
            } else {
                None
            }
        };
        turn.response_body = aggregated.unwrap_or_else(|| self.raw.lock().unwrap().clone());
        if !turn.request_body.is_empty() || !turn.response_body.is_empty() {
            self.insight.submit_turn(turn);
        }
    }
}

struct InsightTapStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, Infallible>> + Send>>,
    state: Arc<InsightTapState>,
}

impl Stream for InsightTapStream {
    type Item = Result<Bytes, Infallible>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = unsafe { self.get_unchecked_mut() };
        match this.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(bytes))) => {
                this.state.push(&bytes);
                Poll::Ready(Some(Ok(bytes)))
            }
            Poll::Ready(None) => {
                this.state.flush();
                Poll::Ready(None)
            }
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(e))),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for InsightTapStream {
    fn drop(&mut self) {
        self.state.flush();
    }
}

/// Accumulate SSE response bytes, aggregate deltas, and submit to AgentMirror when the stream ends.
pub fn wrap_sse_for_insight(
    stream: Pin<Box<dyn Stream<Item = Result<Bytes, Infallible>> + Send>>,
    turn: TraceTurn,
    insight: Arc<InsightService>,
    traffic_max_bytes: usize,
) -> Pin<Box<dyn Stream<Item = Result<Bytes, Infallible>> + Send>> {
    let raw_max = insight_sse_byte_limit(traffic_max_bytes);
    let state = Arc::new(InsightTapState {
        raw: Mutex::new(Vec::new()),
        raw_max,
        line_buf: Mutex::new(String::new()),
        aggregator: Mutex::new(SseInsightAggregator::default()),
        recorded: AtomicBool::new(false),
        turn,
        insight,
    });
    Box::pin(InsightTapStream { inner: stream, state })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insight_sse_limit_is_at_least_8mb() {
        assert_eq!(insight_sse_byte_limit(1024), INSIGHT_SSE_MIN_BYTES);
        assert_eq!(insight_sse_byte_limit(30 * 1024 * 1024), 30 * 1024 * 1024);
    }

    #[test]
    fn aggregator_merges_tool_call_fragments() {
        let mut agg = SseInsightAggregator::default();
        agg.ingest_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"exec","arguments":"{\"path"}}]}}]}"#,
        );
        agg.ingest_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\":\"/tmp/x\"}"}}]}}]}"#,
        );
        agg.ingest_line(r#"data: {"choices":[{"finish_reason":"tool_calls"}]}"#);
        assert!(agg.has_payload());
        let body = agg.to_response_json();
        let json: Value = serde_json::from_slice(&body).unwrap();
        let args = json["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .unwrap();
        assert!(args.contains("/tmp/x"));
    }
}
