use std::sync::Arc;

use crate::critic::{evaluate, CriticInput};
use crate::extract::{drafts_to_events, extract_from_turn, ExtractContext};
use crate::graph::build_graph;
use crate::infer::infer_goal_llm;
use crate::llm::LlmClient;
use crate::models::{EventKind, InsightConfig, RunRecord, RunStatus, TraceTurn};
use crate::parser::{apply_messages_delta, parse_request, parse_response};
use crate::report::{build_reflection_report, outcome_from_status};
use crate::safety::{scan_action_events, SafetyScanner};
use crate::separator::{infer_goal_from_request, new_run_id, resolve_agent, should_start_new_run};
use crate::store::InsightStore;

pub struct Pipeline {
    store: Arc<InsightStore>,
    safety: Option<Arc<dyn SafetyScanner>>,
    llm: Option<Arc<dyn LlmClient>>,
    config: InsightConfig,
}

impl Pipeline {
    pub fn new(
        store: Arc<InsightStore>,
        safety: Option<Arc<dyn SafetyScanner>>,
        llm: Option<Arc<dyn LlmClient>>,
        config: InsightConfig,
    ) -> Self {
        Self {
            store,
            safety,
            llm,
            config,
        }
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

        let full_req = parse_request(&turn.request_body);
        let resp = parse_response(&turn.response_body);

        let existing_agent = {
            let fp_agent = resolve_agent(&turn, &full_req, None);
            self.store.get_agent(&fp_agent.agent_id)?
        };

        let ctx = resolve_agent(&turn, &full_req, existing_agent.as_ref());
        self.store.upsert_agent(&ctx.agent_record)?;

        let active_run = self
            .store
            .find_active_run(&ctx.agent_id, &turn.session_id)?
            .or_else(|| {
                self.store
                    .find_active_run_for_session(&turn.session_id)
                    .ok()
                    .flatten()
            });
        let is_new_run = should_start_new_run(&full_req, active_run.as_ref(), turn.timestamp);

        let mut run = if is_new_run { None } else { active_run };

        if run.is_none() {
            let goal = infer_goal_from_request(&full_req);
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
                messages_seen: 0,
                graph_path: None,
            };
            self.store.insert_run(&record)?;
            run = Some(record);
        }

        let mut run = run.expect("run must exist");
        if run.status != RunStatus::Running {
            run.status = RunStatus::Running;
        }
        run.turn_count += 1;
        run.ended_at = Some(turn.timestamp);

        if run.goal.is_empty() || run.goal == "Unknown task" {
            let g = infer_goal_from_request(&full_req);
            if g != "Unknown task" {
                run.goal = g;
            }
        }

        let req = apply_messages_delta(&full_req, run.messages_seen);
        run.messages_seen = full_req.new_messages.len() as u32;

        let start_seq = self.store.next_event_seq(&run.run_id)?;
        let goal_already_in_run = self.store.list_events(&run.run_id)?.iter().any(|e| {
            matches!(e.kind, EventKind::Goal | EventKind::SubGoal)
        });
        let mut extract_ctx = ExtractContext::from_goal(&run.goal, goal_already_in_run);
        let extracted = extract_from_turn(
            &turn,
            &req,
            &resp,
            &run.run_id,
            start_seq,
            &mut extract_ctx,
        );
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
        if self.config.llm_critic && run.turn_count == 1 {
            let refined = infer_goal_llm(self.llm.as_deref(), &all_events, &run.goal);
            if refined != run.goal {
                run.goal = refined;
            }
        }

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

        let report = build_reflection_report(
            &self.store,
            &run,
            self.safety.as_deref(),
            self.llm.as_deref(),
            self.config.llm_critic,
        )?;
        self.store.save_report(&report)?;

        self.store.mark_audit_processed(&turn.audit_id)?;
        Ok(())
    }
}
