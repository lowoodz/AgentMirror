use std::convert::Infallible;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use bytes::Bytes;
use futures::Stream;
use smr_insight::{InsightService, TraceTurn};

struct InsightTapState {
    collector: Mutex<Vec<u8>>,
    max_bytes: usize,
    recorded: AtomicBool,
    turn: TraceTurn,
    insight: Arc<InsightService>,
}

impl InsightTapState {
    fn push(&self, chunk: &[u8]) {
        let mut buf = self.collector.lock().unwrap();
        if buf.len() < self.max_bytes {
            let take = (self.max_bytes - buf.len()).min(chunk.len());
            buf.extend_from_slice(&chunk[..take]);
        }
    }

    fn flush(&self) {
        if self.recorded.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut turn = self.turn.clone();
        turn.response_body = self.collector.lock().unwrap().clone();
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

/// Accumulate SSE response bytes and submit to AgentMirror when the stream ends.
pub fn wrap_sse_for_insight(
    stream: Pin<Box<dyn Stream<Item = Result<Bytes, Infallible>> + Send>>,
    turn: TraceTurn,
    insight: Arc<InsightService>,
    max_bytes: usize,
) -> Pin<Box<dyn Stream<Item = Result<Bytes, Infallible>> + Send>> {
    let max_bytes = max_bytes.max(1024);
    let state = Arc::new(InsightTapState {
        collector: Mutex::new(Vec::new()),
        max_bytes,
        recorded: AtomicBool::new(false),
        turn,
        insight,
    });
    Box::pin(InsightTapStream { inner: stream, state })
}
