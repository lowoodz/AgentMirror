use crate::models::{CognitiveEvent, EventKind, GraphEdge, GraphNode, ReasoningGraph};

pub fn build_graph(run_id: &str, events: &[CognitiveEvent]) -> ReasoningGraph {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut prev_id: Option<String> = None;

    for (i, event) in events.iter().enumerate() {
        let id = format!("n{i}");
        nodes.push(GraphNode {
            id: id.clone(),
            kind: event.kind.as_str().to_string(),
            label: event.summary.clone(),
            timestamp: Some(event.timestamp),
        });
        if let Some(prev) = prev_id {
            edges.push(GraphEdge {
                from: prev,
                to: id.clone(),
                label: edge_label(event.kind),
            });
        }
        prev_id = Some(id);
    }

    ReasoningGraph {
        run_id: run_id.to_string(),
        nodes,
        edges,
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

    #[test]
    fn builds_linear_graph() {
        let run_id = "r1";
        let events = vec![
            CognitiveEvent {
                id: "e1".into(),
                run_id: run_id.into(),
                seq: 0,
                kind: EventKind::Goal,
                timestamp: Utc::now(),
                summary: "fix bug".into(),
                audit_id: "a1".into(),
                confidence: 1.0,
                metadata: serde_json::Value::Null,
            },
            CognitiveEvent {
                id: "e2".into(),
                run_id: run_id.into(),
                seq: 1,
                kind: EventKind::Action,
                timestamp: Utc::now(),
                summary: "Read(file)".into(),
                audit_id: "a1".into(),
                confidence: 1.0,
                metadata: serde_json::Value::Null,
            },
        ];
        let g = build_graph(run_id, &events);
        assert_eq!(g.nodes.len(), 2);
        assert_eq!(g.edges.len(), 1);
    }
}
