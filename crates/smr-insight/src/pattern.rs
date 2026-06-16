use std::collections::HashMap;

use crate::models::{ActionPattern, RunActionSequence, RunStatus};

const MIN_PATTERN_COUNT: u32 = 2;

pub fn mine_patterns(sequences: &[RunActionSequence]) -> Vec<ActionPattern> {
    let mut bigrams: HashMap<String, (u32, u32)> = HashMap::new();
    let mut trigrams: HashMap<String, (u32, u32)> = HashMap::new();

    for seq in sequences {
        if seq.actions.len() < 2 {
            continue;
        }
        let is_success = seq.status == RunStatus::Completed;
        let is_failure = seq.status == RunStatus::Failed;
        if !is_success && !is_failure {
            continue;
        }
        for window in seq.actions.windows(2) {
            let key = format!("{} → {}", normalize_token(&window[0]), normalize_token(&window[1]));
            let entry = bigrams.entry(key).or_insert((0, 0));
            if is_success {
                entry.0 += 1;
            } else {
                entry.1 += 1;
            }
        }
        if seq.actions.len() >= 3 {
            for window in seq.actions.windows(3) {
                let key = format!(
                    "{} → {} → {}",
                    normalize_token(&window[0]),
                    normalize_token(&window[1]),
                    normalize_token(&window[2])
                );
                let entry = trigrams.entry(key).or_insert((0, 0));
                if is_success {
                    entry.0 += 1;
                } else {
                    entry.1 += 1;
                }
            }
        }
    }

    let mut patterns: Vec<ActionPattern> = Vec::new();
    patterns.extend(collect_patterns(bigrams, 2));
    patterns.extend(collect_patterns(trigrams, 3));
    patterns.sort_by(|a, b| {
        b.success_count
            .saturating_add(b.failure_count)
            .cmp(&a.success_count.saturating_add(a.failure_count))
    });
    patterns.truncate(20);
    patterns
}

fn collect_patterns(map: HashMap<String, (u32, u32)>, step_count: usize) -> Vec<ActionPattern> {
    map.into_iter()
        .filter_map(|(label, (success, failure))| {
            let total = success + failure;
            if total < MIN_PATTERN_COUNT {
                return None;
            }
            let dominant = if success > failure {
                "success"
            } else if failure > success {
                "failure"
            } else {
                "mixed"
            };
            if dominant == "mixed" && total < 3 {
                return None;
            }
            let steps: Vec<String> = label.split(" → ").map(str::to_string).collect();
            if steps.len() != step_count {
                return None;
            }
            Some(ActionPattern {
                steps,
                success_count: success,
                failure_count: failure,
                outcome_hint: dominant.to_string(),
            })
        })
        .collect()
}

fn normalize_token(s: &str) -> String {
    let t = s.trim().to_ascii_lowercase();
    if t.len() <= 48 {
        t
    } else {
        t[..48].to_string()
    }
}

/// True when this run's action sequence contains the pattern's steps in order.
pub fn pattern_matches_run(pattern: &ActionPattern, actions: &[String]) -> bool {
    let n = pattern.steps.len();
    if n == 0 || actions.len() < n {
        return false;
    }
    let norm: Vec<String> = actions.iter().map(|a| normalize_token(a)).collect();
    (0..=norm.len() - n).any(|start| {
        pattern
            .steps
            .iter()
            .enumerate()
            .all(|(j, step)| step_matches(step, &norm[start + j]))
    })
}

fn step_matches(step: &str, action: &str) -> bool {
    action.contains(step) || step.contains(action)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(status: RunStatus, actions: &[&str]) -> RunActionSequence {
        RunActionSequence {
            run_id: "r1".to_string(),
            status,
            actions: actions.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn finds_success_bigram() {
        let sequences = vec![
            seq(RunStatus::Completed, &["read file", "apply patch", "run tests"]),
            seq(RunStatus::Completed, &["read file", "apply patch", "commit"]),
            seq(RunStatus::Failed, &["read file", "apply patch", "run tests"]),
        ];
        let patterns = mine_patterns(&sequences);
        assert!(!patterns.is_empty());
        let read_apply = patterns
            .iter()
            .find(|p| p.steps.len() == 2 && p.steps[0].contains("read") && p.steps[1].contains("apply"));
        assert!(read_apply.is_some());
    }

    #[test]
    fn pattern_matches_run_subsequence() {
        let pattern = ActionPattern {
            steps: vec!["read file".into(), "apply patch".into()],
            success_count: 2,
            failure_count: 0,
            outcome_hint: "success".into(),
        };
        let actions = vec![
            "read file".into(),
            "apply patch".into(),
            "run tests".into(),
        ];
        assert!(pattern_matches_run(&pattern, &actions));
        assert!(!pattern_matches_run(&pattern, &["read file".into(), "run tests".into()]));
    }
}
