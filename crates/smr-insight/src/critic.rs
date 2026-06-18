use crate::locale::ReportLanguage;
use crate::models::{CognitiveEvent, CriticsAnalysis, CriticsScore, EventKind, Issue, RunOutcome, Suggestion};
use crate::rule_baseline;

pub struct CriticInput<'a> {
    pub events: &'a [CognitiveEvent],
    pub turn_count: u32,
    pub goal: &'a str,
    pub safety_findings: &'a [String],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskKind {
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
    language: ReportLanguage,
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
            message: rule_baseline::no_clear_goal_issue(language),
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
            message: rule_baseline::no_verification_issue(language),
            severity: "medium".to_string(),
        });
        suggestions.push(rule_baseline::add_verification_suggestion(language));
    }

    if task_kind == TaskKind::Explore && !has_result && input.turn_count >= 3 {
        suggestions.push(rule_baseline::summarize_findings_suggestion(language));
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
            message: rule_baseline::redundant_actions_issue(language),
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
        suggestions.push(rule_baseline::break_into_subgoals_suggestion(
            language,
            input.turn_count,
        ));
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
                message: rule_baseline::destructive_shell_issue(language),
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

    let analyses = rule_baseline::build_analyses(
        language,
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
        let (score, _, issues, _, _) = evaluate(
            CriticInput {
                events: &events,
                turn_count: 4,
                goal: "收集并对比三种缓存方案",
                safety_findings: &[],
            },
            ReportLanguage::En,
        );
        assert!(score.completeness >= 70);
        assert!(!issues.iter().any(|i| {
            i.message.contains("verification") || i.message.contains("验证")
        }));
    }

    #[test]
    fn rule_analyses_populate_all_dimensions() {
        let events = vec![
            event(EventKind::Goal, "Fix login timeout"),
            event(EventKind::Action, "Read(/var/log/auth.log)"),
        ];
        let (_, analyses, _, _, _) = evaluate(
            CriticInput {
                events: &events,
                turn_count: 3,
                goal: "Fix login timeout",
                safety_findings: &[],
            },
            ReportLanguage::En,
        );
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
        let (_, analyses, _, _, _) = evaluate(
            CriticInput {
                events: &events,
                turn_count: 2,
                goal,
                safety_findings: &[],
            },
            ReportLanguage::Zh,
        );
        assert!(analyses.alignment.contains('…') || analyses.alignment.contains("论文"));
        assert!(analyses.alignment.contains("Taxonomy") || analyses.alignment.contains("查看"));
    }

    #[test]
    fn chinese_rule_baseline_uses_chinese_copy() {
        let events = vec![event(EventKind::Goal, "测试目标")];
        let (_, analyses, issues, _, _) = evaluate(
            CriticInput {
                events: &events,
                turn_count: 1,
                goal: "测试目标",
                safety_findings: &[],
            },
            ReportLanguage::Zh,
        );
        assert!(analyses.alignment.contains("目标"));
        assert!(issues.is_empty());
    }
}
