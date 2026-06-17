use std::collections::HashMap;

use smr_insight::models::RunRiskSummary;
use smr_insight::store::InsightStore;

use crate::audit::RequestAudit;
use crate::storage::AuditStore;

pub fn risk_for_run(store: &InsightStore, audits: &HashMap<String, RequestAudit>, run_id: &str) -> RunRiskSummary {
    let audit_ids = store.audit_ids_for_run(run_id).unwrap_or_default();
    aggregate_risk(&audit_ids, audits)
}

pub fn risk_for_runs(
    audits: &HashMap<String, RequestAudit>,
    run_audit_ids: &HashMap<String, Vec<String>>,
    run_ids: &[String],
) -> HashMap<String, RunRiskSummary> {
    run_ids
        .iter()
        .map(|run_id| {
            let ids = run_audit_ids.get(run_id).map(|v| v.as_slice()).unwrap_or(&[]);
            (run_id.clone(), aggregate_risk(ids, audits))
        })
        .collect()
}

fn aggregate_risk(audit_ids: &[String], audits: &HashMap<String, RequestAudit>) -> RunRiskSummary {
    let mut summary = RunRiskSummary::default();
    for id in audit_ids {
        if let Some(audit) = audits.get(id) {
            summary.dlp_replacements += audit.dlp_replacements;
            summary.safety_blocks += audit.safety_blocks;
            summary.safety_observations += audit.safety_observations;
        }
    }
    summary.high_risk = summary.dlp_replacements > 0
        || summary.safety_blocks > 0
        || summary.safety_observations > 0;
    summary
}

pub fn load_audits_for_ids(audit_store: &AuditStore, ids: &[String]) -> HashMap<String, RequestAudit> {
    audit_store.get_audits_by_ids(ids).unwrap_or_default()
}
