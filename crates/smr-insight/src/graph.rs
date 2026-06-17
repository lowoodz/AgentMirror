//! Heuristic reasoning DAG from cognitive event order (not strict causal inference).
//!
//! Parallel tool calls in the same turn share a parent node; sequential turns chain via
//! decision / chain tip / prior action.

use crate::models::{CognitiveEvent, EventKind, GraphEdge, GraphNode, ReasoningGraph};

pub fn build_graph(run_id: &str, events: &[CognitiveEvent]) -> ReasoningGraph {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut chain_tip: Option<String> = None;
    let mut root_tip: Option<String> = None;
    let mut last_decision: Option<String> = None;
    let mut last_action: Option<String> = None;
    let mut last_action_parent: Option<String> = None;

    for (i, event) in events.iter().enumerate() {
        let id = format!("n{i}");
        nodes.push(GraphNode {
            id: id.clone(),
            kind: event.kind.as_str().to_string(),
            label: event.summary.clone(),
            timestamp: Some(event.timestamp),
        });

        match event.kind {
            EventKind::Goal | EventKind::SubGoal => {
                if let Some(from) = root_tip.clone() {
                    if event.kind == EventKind::SubGoal {
                        push_edge(&mut edges, from, id.clone(), edge_label(event.kind));
                    }
                }
                root_tip = Some(id.clone());
                chain_tip = Some(id.clone());
                last_decision = None;
                last_action = None;
                last_action_parent = None;
            }
            EventKind::Decision => {
                let from = last_action
                    .clone()
                    .or(last_decision.clone())
                    .or(chain_tip.clone());
                if let Some(from) = from {
                    push_edge(&mut edges, from, id.clone(), "decides");
                }
                last_decision = Some(id.clone());
                last_action = None;
                last_action_parent = None;
                chain_tip = Some(id.clone());
            }
            EventKind::Action => {
                let same_turn_parallel = i > 0
                    && events[i - 1].kind == EventKind::Action
                    && events[i - 1].audit_id == event.audit_id;
                let from = if same_turn_parallel {
                    last_action_parent.clone()
                } else {
                    last_decision
                        .clone()
                        .or(chain_tip.clone())
                        .or(last_action.clone())
                };
                if let Some(from) = from {
                    let label = if last_decision.is_some() && !same_turn_parallel {
                        "executes".to_string()
                    } else {
                        edge_label(event.kind)
                    };
                    push_edge(&mut edges, from.clone(), id.clone(), label);
                    if !same_turn_parallel {
                        last_action_parent = Some(from);
                    }
                }
                last_action = Some(id.clone());
            }
            EventKind::Observation => {
                let from = last_action.clone().or(chain_tip.clone());
                if let Some(from) = from {
                    push_edge(&mut edges, from, id.clone(), edge_label(event.kind));
                }
                chain_tip = Some(id.clone());
                last_decision = None;
                last_action_parent = None;
            }
            EventKind::Result | EventKind::Reflection => {
                let from = chain_tip
                    .clone()
                    .or(last_action.clone())
                    .or(last_decision.clone());
                if let Some(from) = from {
                    push_edge(&mut edges, from, id.clone(), edge_label(event.kind));
                }
                chain_tip = Some(id.clone());
                last_decision = None;
                last_action_parent = None;
            }
            EventKind::StateTransition => {
                let from = last_action.clone().or(chain_tip.clone());
                if let Some(from) = from {
                    push_edge(&mut edges, from, id.clone(), edge_label(event.kind));
                }
                chain_tip = Some(id.clone());
                last_action_parent = None;
            }
        }
    }

    ReasoningGraph {
        run_id: run_id.to_string(),
        nodes,
        edges,
        heuristic: true,
    }
}

fn push_edge(edges: &mut Vec<GraphEdge>, from: String, to: String, label: impl Into<String>) {
    if from != to {
        edges.push(GraphEdge {
            from,
            to,
            label: label.into(),
        });
    }
}

fn edge_label(kind: EventKind) -> String {
    match kind {
        EventKind::Goal => "starts".to_string(),
        EventKind::Decision => "decides".to_string(),
        EventKind::Action => "acts".to_string(),
        EventKind::Observation => "observes".to_string(),
        EventKind::Result => "results in".to_string(),
        EventKind::StateTransition => "enters".to_string(),
        EventKind::Reflection => "reflects".to_string(),
        EventKind::SubGoal => "sub-goal".to_string(),
    }
}

pub fn execution_summary(events: &[CognitiveEvent]) -> String {
    events
        .iter()
        .filter(|e| matches!(e.kind, EventKind::Action))
        .map(|e| short_label(&e.summary))
        .take(8)
        .collect::<Vec<_>>()
        .join(" → ")
}

fn short_label(s: &str) -> String {
    if s.chars().count() > 40 {
        format!("{}…", s.chars().take(40).collect::<String>())
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use crate::models::CognitiveEvent;

    fn event(seq: u32, kind: EventKind, summary: &str) -> CognitiveEvent {
        CognitiveEvent {
            id: format!("e{seq}"),
            run_id: "r1".into(),
            seq,
            kind,
            timestamp: Utc::now(),
            summary: summary.into(),
            audit_id: format!("audit-{seq}"),
            confidence: 1.0,
            metadata: serde_json::Value::Null,
        }
    }

    fn event_with_audit(seq: u32, kind: EventKind, summary: &str, audit_id: &str) -> CognitiveEvent {
        let mut e = event(seq, kind, summary);
        e.audit_id = audit_id.into();
        e
    }

    #[test]
    fn builds_linear_graph() {
        let run_id = "r1";
        let events = vec![
            event(0, EventKind::Goal, "fix bug"),
            event(1, EventKind::Action, "Read(file)"),
        ];
        let g = build_graph(run_id, &events);
        assert_eq!(g.nodes.len(), 2);
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.edges[0].from, "n0");
        assert!(g.heuristic);
    }

    #[test]
    fn branches_actions_from_decision() {
        let events = vec![
            event(0, EventKind::Goal, "research"),
            event(1, EventKind::Decision, "search then read"),
            event(2, EventKind::Action, "WebSearch"),
            event(3, EventKind::Action, "Read"),
        ];
        let g = build_graph("r1", &events);
        assert_eq!(g.edges.len(), 3);
        assert!(g.edges.iter().any(|e| e.from == "n1" && e.to == "n2"));
        assert!(g.edges.iter().any(|e| e.from == "n1" && e.to == "n3"));
    }

    #[test]
    fn parallel_actions_same_turn_share_parent() {
        let events = vec![
            event(0, EventKind::Goal, "multi-tool"),
            event_with_audit(1, EventKind::Action, "Read(a)", "turn-1"),
            event_with_audit(2, EventKind::Action, "Read(b)", "turn-1"),
        ];
        let g = build_graph("r1", &events);
        assert!(g.edges.iter().any(|e| e.from == "n0" && e.to == "n1"));
        assert!(g.edges.iter().any(|e| e.from == "n0" && e.to == "n2"));
        assert!(!g.edges.iter().any(|e| e.from == "n1" && e.to == "n2"));
    }

    #[test]
    fn goal_is_root_without_incoming_edge() {
        let events = vec![
            event(0, EventKind::Goal, "调研投资标的"),
            event(1, EventKind::Action, "WebSearch"),
            event(2, EventKind::Observation, "found data"),
        ];
        let g = build_graph("r1", &events);
        assert!(!g.edges.iter().any(|e| e.to == "n0"));
        assert!(g.edges.iter().any(|e| e.from == "n0" && e.to == "n1"));
    }
}
