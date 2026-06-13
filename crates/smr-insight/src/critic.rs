use crate::models::{CriticsScore, EventKind, Issue, RunOutcome, Suggestion};
use crate::models::CognitiveEvent;

pub struct CriticInput<'a> {
    pub events: &'a [CognitiveEvent],
    pub turn_count: u32,
    pub goal: &'a str,
    pub safety_findings: &'a [String],
}

pub fn evaluate(input: CriticInput<'_>) -> (CriticsScore, Vec<Issue>, Vec<Suggestion>, RunOutcome) {
    let mut score = CriticsScore::default();
    let mut issues = Vec::new();
    let mut suggestions = Vec::new();

    let has_goal = input.events.iter().any(|e| e.kind == EventKind::Goal);
    let actions: Vec<_> = input
        .events
        .iter()
        .filter(|e| e.kind == EventKind::Action)
        .collect();
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

    score.completeness = if has_verify { 85 } else if actions.is_empty() { 40 } else { 65 };
    if !has_verify && actions.len() >= 2 {
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
