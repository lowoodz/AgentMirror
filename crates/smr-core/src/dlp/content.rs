use aho_corasick::AhoCorasick;
use anyhow::Result;
use regex::Regex;

use crate::config::{ContentCategory, ContentRule, MatchMode, PipelineConfig};
use crate::dlp::fragment::fragment_meets_threshold;
use crate::dlp::sanitize::{sanitize_range, sanitize_whole};
use crate::dlp::TokenVault;

struct RuleMatcher {
    rule: ContentRule,
}

pub struct ContentDlp {
    rules: Vec<RuleMatcher>,
    automaton: Option<AhoCorasick>,
    secret_rules: Vec<ContentRule>,
    preset_patterns: Vec<(String, Regex)>,
}

const PRESET_SECRETS: &[(&str, &str)] = &[
    ("preset-sk", r"sk-[A-Za-z0-9]{20,}"),
    ("preset-akia", r"AKIA[0-9A-Z]{16}"),
    ("preset-ghp", r"ghp_[A-Za-z0-9]{20,}"),
];

impl ContentDlp {
    pub fn new(rules: &[ContentRule], pipeline: &PipelineConfig) -> Result<Self> {
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

        let mut preset_patterns = Vec::new();
        if pipeline.builtin_credential_presets {
            for (id, pat) in PRESET_SECRETS {
                preset_patterns.push((id.to_string(), Regex::new(pat)?));
            }
        }

        Ok(Self {
            rules,
            automaton,
            secret_rules,
            preset_patterns,
        })
    }

    pub fn sanitize_text(&self, text: &str) -> Result<String> {
        let mut result = text.to_string();

        for rule in &self.secret_rules {
            if result.contains(&rule.value) {
                result = result.replace(&rule.value, &sanitize_whole(&rule.value));
            }
        }

        for (id, re) in &self.preset_patterns {
            for mat in re.find_iter(&result).map(|m| m.as_str().to_string()).collect::<Vec<_>>() {
                result = result.replace(&mat, &sanitize_whole(&mat));
                tracing::debug!(preset = %id, "sanitized builtin credential pattern");
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
                if let Some(rule) = self.find_fragment_rule(mat.pattern().as_usize()) {
                    let matched_len = mat.end() - mat.start();
                    let source_len = rule.value.chars().count();
                    if fragment_meets_threshold(
                        source_len,
                        matched_len,
                        rule.min_fragment_len,
                        rule.min_fragment_ratio,
                    ) {
                        ranges.push((mat.start(), mat.end()));
                    }
                }
            }
            result = apply_ranges(&result, &merge_ranges(ranges));
        }

        Ok(result)
    }

    /// Reversible redaction: replace sensitive spans with session tokens stored in the vault.
    pub fn sanitize_text_reversible(
        &self,
        text: &str,
        session_id: &str,
        vault: &TokenVault,
    ) -> Result<String> {
        let mut result = text.to_string();

        for rule in &self.secret_rules {
            if result.contains(&rule.value) {
                let token = vault.token_for(session_id, &rule.value);
                result = result.replace(&rule.value, &token);
            }
        }

        for (id, re) in &self.preset_patterns {
            for mat in re.find_iter(&result).map(|m| m.as_str().to_string()).collect::<Vec<_>>() {
                let token = vault.token_for(session_id, &mat);
                result = result.replace(&mat, &token);
                tracing::debug!(preset = %id, "tokenized builtin credential pattern");
            }
        }

        for matcher in &self.rules {
            let rule = &matcher.rule;
            if rule.category == ContentCategory::Secret {
                continue;
            }
            if rule.match_mode == MatchMode::Full && result.contains(&rule.value) {
                let token = vault.token_for(session_id, &rule.value);
                result = result.replace(&rule.value, &token);
            }
        }

        if let Some(ac) = &self.automaton {
            let mut ranges: Vec<(usize, usize)> = Vec::new();
            for mat in ac.find_iter(&result) {
                if let Some(rule) = self.find_fragment_rule(mat.pattern().as_usize()) {
                    let matched_len = mat.end() - mat.start();
                    let source_len = rule.value.chars().count();
                    if fragment_meets_threshold(
                        source_len,
                        matched_len,
                        rule.min_fragment_len,
                        rule.min_fragment_ratio,
                    ) {
                        ranges.push((mat.start(), mat.end()));
                    }
                }
            }
            result = apply_ranges_reversible(&result, &merge_ranges(ranges), session_id, vault);
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

fn apply_ranges_reversible(
    text: &str,
    ranges: &[(usize, usize)],
    session_id: &str,
    vault: &TokenVault,
) -> String {
    if ranges.is_empty() {
        return text.to_string();
    }
    let mut result = text.to_string();
    for (start, end) in ranges.iter().rev() {
        let char_start = result[..*start].chars().count();
        let char_end = char_start + result[*start..*end].chars().count();
        let matched: String = result.chars().skip(char_start).take(char_end - char_start).collect();
        let token = vault.token_for(session_id, &matched);
        let prefix: String = result.chars().take(char_start).collect();
        let suffix: String = result.chars().skip(char_end).collect();
        result = format!("{prefix}{token}{suffix}");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ContentCategory, MatchMode, PipelineConfig};

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
        let pipeline = PipelineConfig::default();
        let dlp = ContentDlp::new(&rules, &pipeline).unwrap();
        let out = dlp.sanitize_text("prefix sk-secret-key suffix").unwrap();
        assert!(!out.contains("sk-secret-key"));
        let partial = dlp.sanitize_text("prefix sk-secret suffix").unwrap();
        assert_eq!(partial, "prefix sk-secret suffix");
    }

    #[test]
    fn preset_sk_pattern() {
        let pipeline = PipelineConfig {
            builtin_credential_presets: true,
            ..Default::default()
        };
        let dlp = ContentDlp::new(&[], &pipeline).unwrap();
        let out = dlp
            .sanitize_text("key is sk-abcdefghijklmnopqrstuvwxyz123456")
            .unwrap();
        assert!(!out.contains("sk-abcdefghijklmnopqrstuvwxyz123456"));
    }
}
