use regex::Regex;

use crate::config::{OperationRule, OperationSecurityMode, OperationType};
use smr_protocol::ExtractedText;

pub struct OperationSecurity {
    rules: Vec<CompiledRule>,
    mode: OperationSecurityMode,
}

struct CompiledRule {
    rule: OperationRule,
    matcher: Matcher,
}

enum Matcher {
    Literal(String),
    Regex(Regex),
}

impl OperationSecurity {
    pub fn new(rules: &[OperationRule], mode: OperationSecurityMode) -> anyhow::Result<Self> {
        let mut compiled = Vec::new();
        for rule in rules.iter().filter(|r| r.enabled) {
            let matcher = if rule.object.is_regex {
                Matcher::Regex(Regex::new(&rule.object.pattern)?)
            } else {
                Matcher::Literal(rule.object.pattern.clone())
            };
            compiled.push(CompiledRule {
                rule: rule.clone(),
                matcher,
            });
        }
        Ok(Self {
            rules: compiled,
            mode,
        })
    }

    pub fn process_response(
        &self,
        extracted: &[ExtractedText],
    ) -> anyhow::Result<Vec<(ExtractedText, String)>> {
        let mut replacements = Vec::new();
        for item in extracted {
            if let Some(blocked) = self.check_text(&item.text) {
                if self.mode == OperationSecurityMode::Enforce {
                    replacements.push((item.clone(), blocked));
                } else {
                    tracing::warn!(
                        rule_id = %self.last_matched_rule_id(&item.text).unwrap_or_default(),
                        "operation security observe: dangerous output detected"
                    );
                }
            }
        }
        Ok(replacements)
    }

    fn last_matched_rule_id(&self, text: &str) -> Option<String> {
        for compiled in &self.rules {
            if self.matches_rule(text, compiled) {
                return Some(compiled.rule.id.clone());
            }
        }
        None
    }

    fn check_text(&self, text: &str) -> Option<String> {
        for compiled in &self.rules {
            if self.matches_rule(text, compiled) {
                let msg = format!(
                    "[SMR BLOCKED] 操作「{:?}: {}」已被安全策略拦截。规则 ID: {}",
                    compiled.rule.operation, compiled.rule.object.pattern, compiled.rule.id
                );
                return Some(wrap_blocked_payload(text, &msg, compiled.rule.operation));
            }
        }
        None
    }

    fn matches_rule(&self, text: &str, compiled: &CompiledRule) -> bool {
        let pattern_matches = match &compiled.matcher {
            Matcher::Literal(p) => text.contains(p.as_str()),
            Matcher::Regex(re) => re.is_match(text),
        };
        if !pattern_matches {
            return false;
        }
        match compiled.rule.operation {
            OperationType::CommandExec => is_command_exec(text),
            OperationType::ApiCall => is_api_call(text),
            OperationType::NetworkAccess => is_network_access(text),
        }
    }
}

fn is_command_exec(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("run_terminal_cmd")
        || lower.contains("bash")
        || lower.contains("shell")
        || lower.contains("\"command\"")
        || lower.contains("rm -rf")
        || lower.contains("sudo ")
}

fn is_api_call(text: &str) -> bool {
    text.contains("http://") || text.contains("https://") || text.contains("fetch(")
}

fn is_network_access(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("curl ")
        || lower.contains("wget ")
        || lower.contains("nc ")
        || lower.contains("http://")
        || lower.contains("https://")
}

fn wrap_blocked_payload(_original: &str, message: &str, op: OperationType) -> String {
    if _original.trim_start().starts_with('{') {
        serde_json::json!({
            "smr_blocked": true,
            "operation": format!("{:?}", op),
            "message": message,
        })
        .to_string()
    } else {
        message.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{OperationObject, OperationRule, OperationType};

    #[test]
    fn blocks_rm_rf_in_tool_output() {
        let rules = vec![OperationRule {
            id: "block-rm".into(),
            enabled: true,
            operation: OperationType::CommandExec,
            object: OperationObject {
                pattern: "rm -rf".into(),
                is_regex: false,
            },
        }];
        let ops = OperationSecurity::new(&rules, OperationSecurityMode::Enforce).unwrap();
        let extracted = vec![ExtractedText {
            pointer: smr_protocol::TextPointer::OpenAiToolCallArguments {
                message_index: 0,
                tool_index: 0,
            },
            text: r#"{"command":"rm -rf /"}"#.into(),
        }];
        let out = ops.process_response(&extracted).unwrap();
        assert_eq!(out.len(), 1);
        assert!(out[0].1.contains("SMR BLOCKED"));
    }
}
