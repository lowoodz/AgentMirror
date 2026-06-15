use crate::models::{CognitiveEvent, CriticsAnalysis, CriticsScore, EventKind, Issue, RunOutcome, Suggestion};

pub struct CriticInput<'a> {
    pub events: &'a [CognitiveEvent],
    pub turn_count: u32,
    pub goal: &'a str,
    pub safety_findings: &'a [String],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskKind {
    Explore,
    Coding,
    Chat,
}

fn infer_task_kind(events: &[CognitiveEvent]) -> TaskKind {
    let has_coding_actions = events.iter().any(|e| {
        e.kind == EventKind::Action
            && (e.summary.contains("Edit")
                || e.summary.contains("Write(")
                || e.summary.contains("patch")
                || e.summary.contains("ApplyPatch"))
    });
    if has_coding_actions {
        return TaskKind::Coding;
    }
    if events.iter().any(|e| e.kind == EventKind::Action) {
        return TaskKind::Explore;
    }
    TaskKind::Chat
}

pub fn evaluate(
    input: CriticInput<'_>,
) -> (
    CriticsScore,
    CriticsAnalysis,
    Vec<Issue>,
    Vec<Suggestion>,
    RunOutcome,
) {
    let mut score = CriticsScore::default();
    let mut issues = Vec::new();
    let mut suggestions = Vec::new();

    let task_kind = infer_task_kind(input.events);
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
        TaskKind::Explore => {
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

    if task_kind == TaskKind::Explore && !has_result && input.turn_count >= 3 {
        suggestions.push(Suggestion {
            message: "Summarize findings with an explicit conclusion for the original goal".to_string(),
            rationale: "Multi-step agent runs are easier to review when they end with a clear outcome"
                .to_string(),
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

    let analyses = build_critic_analyses(
        &score,
        input,
        task_kind,
        has_goal,
        has_result,
        has_verify,
        has_observation,
        actions.len(),
        unique_actions.len(),
    );

    (score, analyses, issues, suggestions, outcome)
}

fn build_critic_analyses(
    score: &CriticsScore,
    input: CriticInput<'_>,
    task_kind: TaskKind,
    has_goal: bool,
    has_result: bool,
    has_verify: bool,
    has_observation: bool,
    action_count: usize,
    unique_action_count: usize,
) -> CriticsAnalysis {
    let goal_snip = truncate_chars(input.goal, 120);

    let alignment = if has_goal {
        format!(
            "A goal was recorded (\"{}\"). With {} action(s) across {} turn(s), review whether each step still serves this objective and context — score {} suggests {} alignment.",
            goal_snip,
            action_count,
            input.turn_count,
            score.alignment,
            alignment_label(score.alignment)
        )
    } else {
        "No clear goal was extracted from the trace, so actions cannot be reliably judged against stated intent. The agent may be drifting or the session goal was never captured.".to_string()
    };

    let necessity = if score.necessity < 60 {
        format!(
            "Detected {} total actions but only {} distinct action patterns — likely redundant or repeated steps. Score {} indicates unnecessary repetition that could be trimmed.",
            action_count,
            unique_action_count,
            score.necessity
        )
    } else if action_count == 0 {
        "No tool actions were recorded; necessity is moot until the agent executes steps toward the goal.".to_string()
    } else {
        format!(
            "Recorded {} action(s) with {} distinct patterns — no major redundancy detected (score {}). Each step appears reasonably necessary for the current trajectory.",
            action_count,
            unique_action_count,
            score.necessity
        )
    };

    let completeness = match task_kind {
        TaskKind::Explore => {
            if has_result {
                format!(
                    "Exploration run ended with an explicit result/conclusion (score {}). The approach appears to cover investigation and synthesis for the goal.",
                    score.completeness
                )
            } else if has_observation && action_count > 0 {
                format!(
                    "The agent gathered observations via {} action(s) but no final result/conclusion was extracted (score {}). Consider whether analysis and a definitive answer to the goal are missing.",
                    action_count,
                    score.completeness
                )
            } else {
                format!(
                    "Exploration appears incomplete — few observations or actions relative to the goal (score {}). Key investigation phases may be missing.",
                    score.completeness
                )
            }
        }
        TaskKind::Coding => {
            if has_verify {
                format!(
                    "Implementation was followed by a verification step (score {}). The coding workflow includes validation before closure.",
                    score.completeness
                )
            } else if action_count >= 2 {
                format!(
                    "Implementation actions were recorded but no verification/test step was detected (score {}). The fix may be incomplete without validation.",
                    score.completeness
                )
            } else {
                format!(
                    "Coding task with limited action evidence (score {}). Plan, implement, and verify phases may not all be present.",
                    score.completeness
                )
            }
        }
        TaskKind::Chat => {
            if has_result {
                format!(
                    "Conversation reached a stated outcome (score {}). The dialogue appears to resolve the user's request.",
                    score.completeness
                )
            } else {
                format!(
                    "Multi-turn chat without a clear extracted result (score {}). The response may be partial or still in progress.",
                    score.completeness
                )
            }
        }
    };

    let efficiency = match input.turn_count {
        0..=5 => format!(
            "Used {} LLM turn(s) — compact execution (score {}). Path length looks reasonable for the scope.",
            input.turn_count,
            score.efficiency
        ),
        6..=15 => format!(
            "{} turns consumed (score {}). Monitor for detours; scope may still be acceptable.",
            input.turn_count,
            score.efficiency
        ),
        16..=30 => format!(
            "{} turns is relatively heavy (score {}). The agent may be taking indirect routes or re-planning often — consider sub-goals or tighter prompts.",
            input.turn_count,
            score.efficiency
        ),
        _ => format!(
            "{} turns indicates a long, potentially inefficient path (score {}). Breaking the task into smaller runs would improve reviewability and cost.",
            input.turn_count,
            score.efficiency
        ),
    };

    let mut safety_parts: Vec<String> = Vec::new();
    if input.safety_findings.is_empty() {
        safety_parts.push("No policy or DLP safety findings were flagged for this run.".to_string());
    } else {
        safety_parts.push(format!(
            "Policy/DLP flagged {} issue(s): {}.",
            input.safety_findings.len(),
            input.safety_findings.join("; ")
        ));
    }
    if score.safety < 70 {
        safety_parts.push(
            "Potentially risky tool usage (destructive commands or sensitive operations) was detected — review action summaries before replay."
                .to_string(),
        );
    } else {
        safety_parts.push(format!(
            "Overall safety score {} — no high-risk patterns beyond routine agent tooling.",
            score.safety
        ));
    }

    CriticsAnalysis {
        alignment,
        necessity,
        completeness,
        efficiency,
        safety: safety_parts.join(" "),
    }
}

fn alignment_label(score: u8) -> &'static str {
    match score {
        0..=49 => "weak",
        50..=69 => "moderate",
        70..=84 => "good",
        _ => "strong",
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
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
    fn explore_run_skips_verification_penalty() {
        let events = vec![
            event(EventKind::Goal, "收集并对比三种缓存方案"),
            event(EventKind::Action, "WebSearch(redis vs memcached)"),
            event(EventKind::Observation, "found comparison articles"),
        ];
        let (score, _, issues, _, _) = evaluate(CriticInput {
            events: &events,
            turn_count: 4,
            goal: "收集并对比三种缓存方案",
            safety_findings: &[],
        });
        assert!(score.completeness >= 70);
        assert!(!issues.iter().any(|i| i.message.contains("verification")));
    }

    #[test]
    fn rule_analyses_populate_all_dimensions() {
        let events = vec![
            event(EventKind::Goal, "Fix login timeout"),
            event(EventKind::Action, "Read(/var/log/auth.log)"),
        ];
        let (_, analyses, _, _, _) = evaluate(CriticInput {
            events: &events,
            turn_count: 3,
            goal: "Fix login timeout",
            safety_findings: &[],
        });
        assert!(analyses.alignment.contains("Fix login"));
        assert!(!analyses.necessity.is_empty());
        assert!(!analyses.completeness.is_empty());
        assert!(!analyses.efficiency.is_empty());
        assert!(!analyses.safety.is_empty());
    }

    #[test]
    fn analyses_truncate_cjk_goal_without_panic() {
        let goal = "查看 A Taxonomy of Network Threats and the Effect of Current Datasets on Intrusion Detection Systems, IEEE.pdf 论文的摘要";
        let events = vec![event(EventKind::Goal, goal)];
        let (_, analyses, _, _, _) = evaluate(CriticInput {
            events: &events,
            turn_count: 2,
            goal,
            safety_findings: &[],
        });
        assert!(analyses.alignment.contains('…') || analyses.alignment.contains("论文"));
        assert!(analyses.alignment.contains("Taxonomy") || analyses.alignment.contains("查看"));
    }
}
