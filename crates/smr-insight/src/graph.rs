use crate::models::{CognitiveEvent, EventKind, GraphEdge, GraphNode, ReasoningGraph};

pub fn build_graph(run_id: &str, events: &[CognitiveEvent]) -> ReasoningGraph {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut chain_tip: Option<String> = None;
    let mut last_decision: Option<String> = None;
    let mut last_action: Option<String> = None;

    for (i, event) in events.iter().enumerate() {
        let id = format!("n{i}");
        nodes.push(GraphNode {
            id: id.clone(),
            kind: event.kind.as_str().to_string(),
            label: event.summary.clone(),
            timestamp: Some(event.timestamp),
        });

        match event.kind {
            EventKind::Decision => {
                if let Some(from) = chain_tip.clone().or(last_action.clone()) {
                    push_edge(&mut edges, from, id.clone(), "decides");
                }
                last_decision = Some(id.clone());
                last_action = None;
                chain_tip = Some(id.clone());
            }
            EventKind::Action => {
                let from = last_decision
                    .clone()
                    .or(chain_tip.clone())
                    .or(last_action.clone());
                if let Some(from) = from {
                    let label = if last_decision.is_some() {
                        "executes".to_string()
                    } else {
                        edge_label(event.kind)
                    };
                    push_edge(&mut edges, from, id.clone(), label);
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
            }
            EventKind::Goal | EventKind::SubGoal | EventKind::Result | EventKind::Reflection => {
                if let Some(from) = chain_tip.clone().or(last_action.clone()) {
                    push_edge(&mut edges, from, id.clone(), edge_label(event.kind));
                }
                chain_tip = Some(id.clone());
                last_decision = None;
            }
            EventKind::StateTransition => {
                let from = last_action.clone().or(chain_tip.clone());
                if let Some(from) = from {
                    push_edge(&mut edges, from, id.clone(), edge_label(event.kind));
                }
                chain_tip = Some(id.clone());
            }
        }
    }

    ReasoningGraph {
        run_id: run_id.to_string(),
        nodes,
        edges,
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
            audit_id: "a1".into(),
            confidence: 1.0,
            metadata: serde_json::Value::Null,
        }
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
}
