use aho_corasick::AhoCorasick;
use anyhow::Result;

use crate::config::{ContentCategory, ContentRule, MatchMode};
use crate::dlp::sanitize::{sanitize_range, sanitize_whole};

struct RuleMatcher {
    rule: ContentRule,
}

pub struct ContentDlp {
    rules: Vec<RuleMatcher>,
    automaton: Option<AhoCorasick>,
    secret_rules: Vec<ContentRule>,
}

impl ContentDlp {
    pub fn new(rules: &[ContentRule]) -> Result<Self> {
        let active: Vec<ContentRule> = rules.iter().filter(|r| r.enabled).cloned().collect();
        let fragment_values: Vec<String> = active
            .iter()
            .filter(|r| r.category != ContentCategory::Secret && r.match_mode == MatchMode::Fragment)
            .map(|r| r.value.clone())
            .collect();
        let automaton = if fragment_values.is_empty() {
            None
        } else {
            Some(AhoCorasick::new(&fragment_values)?)
        };
        let secret_rules: Vec<ContentRule> = active
            .iter()
            .filter(|r| r.category == ContentCategory::Secret)
            .cloned()
            .collect();
        let rules = active.into_iter().map(|rule| RuleMatcher { rule }).collect();
        Ok(Self {
            rules,
            automaton,
            secret_rules,
        })
    }

    pub fn sanitize_text(&self, text: &str) -> Result<String> {
        let mut result = text.to_string();

        for rule in &self.secret_rules {
            if result.contains(&rule.value) {
                result = result.replace(&rule.value, &sanitize_whole(&rule.value));
            }
        }

        for matcher in &self.rules {
            let rule = &matcher.rule;
            if rule.category == ContentCategory::Secret {
                continue;
            }
            if rule.match_mode == MatchMode::Full && result.contains(&rule.value) {
                result = result.replace(&rule.value, &sanitize_whole(&rule.value));
            }
        }

        if let Some(ac) = &self.automaton {
            let mut ranges: Vec<(usize, usize)> = Vec::new();
            for mat in ac.find_iter(&result) {
                let rule = self.find_fragment_rule(mat.pattern().as_usize());
                if let Some(rule) = rule {
                    let len = rule.value.chars().count();
                    let min_len = rule
                        .min_fragment_len
                        .unwrap_or(8)
                        .max(1);
                    if len >= min_len {
                        let start_byte = mat.start();
                        let end_byte = mat.end();
                        ranges.push((start_byte, end_byte));
                    }
                }
            }
            result = apply_ranges(&result, &merge_ranges(ranges));
        }

        Ok(result)
    }

    fn find_fragment_rule(&self, pattern_index: usize) -> Option<&ContentRule> {
        let mut idx = 0;
        for matcher in &self.rules {
            let rule = &matcher.rule;
            if rule.category != ContentCategory::Secret && rule.match_mode == MatchMode::Fragment {
                if idx == pattern_index {
                    return Some(rule);
                }
                idx += 1;
            }
        }
        None
    }
}

fn merge_ranges(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    if ranges.is_empty() {
        return ranges;
    }
    ranges.sort_by_key(|r| r.0);
    let mut merged = vec![ranges[0]];
    for (start, end) in ranges.into_iter().skip(1) {
        let last = merged.last_mut().unwrap();
        if start <= last.1 {
            last.1 = last.1.max(end);
        } else {
            merged.push((start, end));
        }
    }
    merged
}

fn apply_ranges(text: &str, ranges: &[(usize, usize)]) -> String {
    if ranges.is_empty() {
        return text.to_string();
    }
    let mut result = text.to_string();
    for (start, end) in ranges.iter().rev() {
        let char_start = result[..*start].chars().count();
        let char_end = char_start + result[*start..*end].chars().count();
        result = sanitize_range(&result, char_start, char_end);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ContentCategory, MatchMode};

    #[test]
    fn secret_requires_full_match() {
        let rules = vec![ContentRule {
            id: "s1".into(),
            enabled: true,
            match_mode: MatchMode::Full,
            value: "sk-secret-key".into(),
            category: ContentCategory::Secret,
            min_fragment_len: None,
            min_fragment_ratio: None,
        }];
        let dlp = ContentDlp::new(&rules).unwrap();
        let out = dlp.sanitize_text("prefix sk-secret-key suffix").unwrap();
        assert!(!out.contains("sk-secret-key"));
        let partial = dlp.sanitize_text("prefix sk-secret suffix").unwrap();
        assert_eq!(partial, "prefix sk-secret suffix");
    }
}
