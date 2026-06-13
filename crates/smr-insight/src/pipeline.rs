use std::sync::Arc;

use crate::critic::{evaluate, CriticInput};
use crate::extract::{drafts_to_events, extract_from_turn};
use crate::graph::build_graph;
use crate::models::{RunRecord, RunStatus, TraceTurn};
use crate::parser::{parse_request, parse_response};
use crate::report::{build_reflection_report, outcome_from_status};
use crate::safety::{scan_action_events, SafetyScanner};
use crate::separator::{infer_goal_from_request, new_run_id, resolve_agent, should_start_new_run};
use crate::store::InsightStore;

pub struct Pipeline {
    store: Arc<InsightStore>,
    safety: Option<Arc<dyn SafetyScanner>>,
}

impl Pipeline {
    pub fn new(store: Arc<InsightStore>, safety: Option<Arc<dyn SafetyScanner>>) -> Self {
        Self { store, safety }
    }

    pub fn store(&self) -> &InsightStore {
        &self.store
    }

    pub fn process_turn(&self, turn: TraceTurn) -> anyhow::Result<()> {
        if turn.request_body.is_empty() && turn.response_body.is_empty() {
            return Ok(());
        }
        if self.store.is_audit_processed(&turn.audit_id)? {
            return Ok(());
        }

        let req = parse_request(&turn.request_body);
        let resp = parse_response(&turn.response_body);

        let existing_agent = {
            let fp_agent = resolve_agent(&turn, &req, None);
            self.store.get_agent(&fp_agent.agent_id)?
        };

        let ctx = resolve_agent(&turn, &req, existing_agent.as_ref());
        self.store.upsert_agent(&ctx.agent_record)?;

        let active_run = self
            .store
            .find_active_run(&ctx.agent_id, &turn.session_id)?;
        let is_new_run = should_start_new_run(&req, active_run.as_ref(), turn.timestamp);

        let mut run = if is_new_run {
            None
        } else {
            active_run
        };

        if run.is_none() {
            let goal = infer_goal_from_request(&req);
            let run_id = new_run_id(&turn.session_id, &ctx.agent_id);
            let record = RunRecord {
                run_id: run_id.clone(),
                agent_id: ctx.agent_id.clone(),
                session_id: turn.session_id.clone(),
                started_at: turn.timestamp,
                ended_at: None,
                status: RunStatus::Running,
                goal,
                turn_count: 0,
                graph_path: None,
            };
            self.store.insert_run(&record)?;
            run = Some(record);
        }

        let mut run = run.expect("run must exist");
        run.turn_count += 1;
        run.ended_at = Some(turn.timestamp);

        if run.goal.is_empty() || run.goal == "Unknown task" {
            let g = infer_goal_from_request(&req);
            if g != "Unknown task" {
                run.goal = g;
            }
        }

        let start_seq = self.store.next_event_seq(&run.run_id)?;
        let extracted = extract_from_turn(&turn, &req, &resp, &run.run_id, start_seq);
        let events = drafts_to_events(
            extracted.events,
            &run.run_id,
            &turn.audit_id,
            start_seq,
            turn.timestamp,
        );
        for event in events {
            self.store.insert_event(&event)?;
        }

        let all_events = self.store.list_events(&run.run_id)?;
        let graph = build_graph(&run.run_id, &all_events);
        let graph_json = serde_json::to_string_pretty(&graph)?;
        let graph_path = self.store.save_graph_json(&run.run_id, &graph_json)?;
        run.graph_path = Some(graph_path);

        let safety_findings = scan_action_events(&all_events, self.safety.as_deref());
        let (_, _, _, outcome) = evaluate(CriticInput {
            events: &all_events,
            turn_count: run.turn_count,
            goal: &run.goal,
            safety_findings: &safety_findings,
        });
        run.status = outcome_from_status(run.status, outcome);

        self.store.update_run(&run)?;

        let report = build_reflection_report(&self.store, &run, self.safety.as_deref())?;
        self.store.save_report(&report)?;

        self.store.mark_audit_processed(&turn.audit_id)?;
        Ok(())
    }
}
