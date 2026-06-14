use crate::extract::is_research_goal;
use crate::models::{CognitiveEvent, CriticsScore, EventKind, Issue, RunOutcome, Suggestion};

pub struct CriticInput<'a> {
    pub events: &'a [CognitiveEvent],
    pub turn_count: u32,
    pub goal: &'a str,
    pub safety_findings: &'a [String],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskKind {
    Research,
    Coding,
    Chat,
}

fn infer_task_kind(goal: &str, events: &[CognitiveEvent]) -> TaskKind {
    if is_research_goal(goal) {
        return TaskKind::Research;
    }
    let has_research_actions = events.iter().any(|e| {
        e.kind == EventKind::Action
            && (e.summary.starts_with("WebSearch")
                || e.summary.contains("search")
                || e.summary.contains("调研"))
    });
    if has_research_actions {
        return TaskKind::Research;
    }
    let has_coding_actions = events.iter().any(|e| {
        e.kind == EventKind::Action
            && (e.summary.contains("Edit")
                || e.summary.contains("Read(")
                || e.summary.contains("Bash")
                || e.summary.contains("patch"))
    });
    if has_coding_actions {
        TaskKind::Coding
    } else if events.iter().any(|e| e.kind == EventKind::Action) {
        TaskKind::Research
    } else {
        TaskKind::Chat
    }
}

pub fn evaluate(input: CriticInput<'_>) -> (CriticsScore, Vec<Issue>, Vec<Suggestion>, RunOutcome) {
    let mut score = CriticsScore::default();
    let mut issues = Vec::new();
    let mut suggestions = Vec::new();

    let task_kind = infer_task_kind(input.goal, input.events);
    let has_goal = input
        .events
        .iter()
        .any(|e| matches!(e.kind, EventKind::Goal | EventKind::SubGoal));
    let actions: Vec<_> = input
        .events
        .iter()
        .filter(|e| e.kind == EventKind::Action)
        .collect();
    let has_observation = input
        .events
        .iter()
        .any(|e| e.kind == EventKind::Observation);
    let has_verify = input.events.iter().any(|e| {
        e.kind == EventKind::StateTransition
            && e.summary.to_ascii_lowercase().contains("verification")
    });
    let has_result = input
        .events
        .iter()
        .any(|e| e.kind == EventKind::Result);

    score.alignment = if has_goal { 85 } else { 55 };
    if !has_goal {
        issues.push(Issue {
            message: "No clear goal detected in conversation".to_string(),
            severity: "medium".to_string(),
        });
    }

    score.completeness = match task_kind {
        TaskKind::Research => {
            if has_result {
                90
            } else if has_observation && !actions.is_empty() {
                75
            } else if actions.is_empty() {
                40
            } else {
                60
            }
        }
        TaskKind::Coding => {
            if has_verify {
                85
            } else if actions.is_empty() {
                40
            } else {
                65
            }
        }
        TaskKind::Chat => {
            if has_result {
                85
            } else if input.turn_count <= 2 {
                70
            } else {
                60
            }
        }
    };

    if task_kind == TaskKind::Coding && !has_verify && actions.len() >= 2 {
        issues.push(Issue {
            message: "No verification step detected after implementation actions".to_string(),
            severity: "medium".to_string(),
        });
        suggestions.push(Suggestion {
            message: "Add a test or validation step before marking the task complete".to_string(),
            rationale: "Bugfix and coding tasks benefit from explicit verification".to_string(),
            priority: "high".to_string(),
        });
    }

    if task_kind == TaskKind::Research && !has_result && input.turn_count >= 3 {
        suggestions.push(Suggestion {
            message: "Summarize findings with an explicit investment or research conclusion".to_string(),
            rationale: "Research runs are easier to review when they end with a clear recommendation".to_string(),
            priority: "medium".to_string(),
        });
    }

    let unique_actions: std::collections::HashSet<_> =
        actions.iter().map(|a| a.summary.as_str()).collect();
    score.necessity = if actions.len() > unique_actions.len() * 2 {
        50
    } else {
        80
    };
    if score.necessity < 60 {
        issues.push(Issue {
            message: "Repeated similar actions detected — possible redundancy".to_string(),
            severity: "low".to_string(),
        });
    }

    score.efficiency = match input.turn_count {
        0..=5 => 90,
        6..=15 => 75,
        16..=30 => 60,
        _ => 45,
    };
    if input.turn_count > 20 {
        suggestions.push(Suggestion {
            message: "Consider breaking the task into smaller sub-goals".to_string(),
            rationale: format!("Run used {} LLM turns", input.turn_count),
            priority: "medium".to_string(),
        });
    }

    score.safety = 90;
    for finding in input.safety_findings {
        score.safety = score.safety.min(35);
        issues.push(Issue {
            message: finding.clone(),
            severity: "high".to_string(),
        });
    }
    for action in &actions {
        let lower = action.summary.to_ascii_lowercase();
        if lower.contains("rm -rf") || lower.contains("delete") && lower.contains("all") {
            score.safety = score.safety.min(40);
            issues.push(Issue {
                message: "Potentially destructive shell action detected".to_string(),
                severity: "high".to_string(),
            });
        }
    }

    let outcome = if has_result {
        RunOutcome::Success
    } else if input.turn_count == 0 {
        RunOutcome::Unknown
    } else if score.completeness >= 70 {
        RunOutcome::Partial
    } else {
        RunOutcome::Unknown
    };

    (score, issues, suggestions, outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn event(kind: EventKind, summary: &str) -> CognitiveEvent {
        CognitiveEvent {
            id: Uuid::new_v4().to_string(),
            run_id: "r1".into(),
            seq: 0,
            kind,
            timestamp: Utc::now(),
            summary: summary.into(),
            audit_id: "a1".into(),
            confidence: 1.0,
            metadata: serde_json::Value::Null,
        }
    }

    #[test]
    fn research_run_skips_verification_penalty() {
        let events = vec![
            event(EventKind::Goal, "调研珠海金智维是否值得投资"),
            event(EventKind::Action, "WebSearch(金智维)"),
            event(EventKind::Observation, "公司成立于2010年"),
        ];
        let (score, issues, _, _) = evaluate(CriticInput {
            events: &events,
            turn_count: 4,
            goal: "调研珠海金智维是否值得投资",
            safety_findings: &[],
        });
        assert!(score.completeness >= 70);
        assert!(!issues.iter().any(|i| i.message.contains("verification")));
    }
}
