use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::Serialize;

#[derive(Debug, Default)]
pub struct InsightMetrics {
    pub processed: AtomicU64,
    pub dropped: AtomicU64,
    pub spilled: AtomicU64,
    pub spill_recovered: AtomicU64,
    pub llm_jobs_enqueued: AtomicU64,
    pub llm_jobs_completed: AtomicU64,
    pub llm_jobs_failed: AtomicU64,
    pub queue_depth: AtomicU64,
    pub critic_queue_depth: AtomicU64,
    pub activity_seq: AtomicU64,
}

impl InsightMetrics {
    pub fn bump_activity(&self) {
        self.activity_seq.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(
        &self,
        spill_pending: u64,
        oldest_spill_age_secs: Option<u64>,
    ) -> InsightMetricsSnapshot {
        InsightMetricsSnapshot {
            processed: self.processed.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
            spilled: self.spilled.load(Ordering::Relaxed),
            spill_recovered: self.spill_recovered.load(Ordering::Relaxed),
            spill_pending,
            oldest_spill_age_secs,
            queue_depth: self.queue_depth.load(Ordering::Relaxed),
            critic_queue_depth: self.critic_queue_depth.load(Ordering::Relaxed),
            llm_jobs_enqueued: self.llm_jobs_enqueued.load(Ordering::Relaxed),
            llm_jobs_completed: self.llm_jobs_completed.load(Ordering::Relaxed),
            llm_jobs_failed: self.llm_jobs_failed.load(Ordering::Relaxed),
            activity_seq: self.activity_seq.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct InsightMetricsSnapshot {
    pub processed: u64,
    pub dropped: u64,
    pub spilled: u64,
    pub spill_recovered: u64,
    pub spill_pending: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oldest_spill_age_secs: Option<u64>,
    pub queue_depth: u64,
    pub critic_queue_depth: u64,
    pub llm_jobs_enqueued: u64,
    pub llm_jobs_completed: u64,
    pub llm_jobs_failed: u64,
    pub activity_seq: u64,
}

pub type SharedMetrics = Arc<InsightMetrics>;
