use serde::{Deserialize, Serialize};
use smr_insight::{ResetStats, TraceTurn};

use crate::state::SharedApp;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ReplayStats {
    pub reset: ResetStats,
    pub submitted: usize,
    pub skipped_no_traffic: usize,
    pub skipped_not_success: usize,
    pub skipped_empty_body: usize,
    pub errors: usize,
    pub runs_llm_finalized: usize,
}

pub fn reset_insight(app: &SharedApp) -> anyhow::Result<ResetStats> {
    app.insight.reset()
}

pub fn replay_from_traffic(app: &SharedApp, limit: usize) -> anyhow::Result<ReplayStats> {
    let reset = app.insight.reset()?;
    let limit = limit.clamp(1, 10_000);
    let audits = app.storage.list_audits_chronological(limit)?;

    let mut stats = ReplayStats {
        reset,
        ..Default::default()
    };

    for audit in audits {
        if !audit.success {
            stats.skipped_not_success += 1;
            continue;
        }
        let Some((request_body, response_body)) = app.traffic.bodies_for_audit(&audit.id) else {
            stats.skipped_no_traffic += 1;
            continue;
        };
        if request_body.is_empty() && response_body.is_empty() {
            stats.skipped_empty_body += 1;
            continue;
        }
        let turn = TraceTurn {
            audit_id: audit.id.clone(),
            session_id: audit.session_id,
            agent_header: None,
            timestamp: audit.timestamp,
            request_body,
            response_body,
        };
        match app.insight.process_turn_sync(turn) {
            Ok(()) => stats.submitted += 1,
            Err(err) => {
                tracing::warn!(?err, audit_id = %audit.id, "AgentMirror replay failed");
                stats.errors += 1;
            }
        }
    }

    if stats.submitted > 0 {
        match app.insight.finalize_replayed_runs() {
            Ok(n) => stats.runs_llm_finalized = n,
            Err(err) => {
                tracing::warn!(?err, "AgentMirror replay LLM finalization failed");
            }
        }
    }

    Ok(stats)
}
