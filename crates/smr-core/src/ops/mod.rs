use regex::Regex;

use crate::config::{OperationRule, OperationSecurityMode, OperationType, PathProtectionRule};
use smr_protocol::ExtractedText;

mod path_protection;
use path_protection::{level_label, PathProtection};

pub struct OperationSecurity {
    rules: Vec<CompiledRule>,
    path_protection: PathProtection,
    operation_mode: OperationSecurityMode,
    path_protection_mode: OperationSecurityMode,
}

struct CompiledRule {
    rule: OperationRule,
    matcher: Matcher,
}

enum Matcher {
    Literal(String),
    Regex(Regex),
}

enum SecurityMatch {
    Operation {
        payload: String,
        rule_id: String,
    },
    PathProtection {
        payload: String,
        rule_id: String,
    },
}

impl OperationSecurity {
    pub fn new(
        rules: &[OperationRule],
        path_rules: &[PathProtectionRule],
        operation_mode: OperationSecurityMode,
        path_protection_mode: OperationSecurityMode,
    ) -> anyhow::Result<Self> {
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
            path_protection: PathProtection::new(path_rules),
            operation_mode,
            path_protection_mode,
        })
    }

    pub fn process_response(
        &self,
        extracted: &[ExtractedText],
    ) -> anyhow::Result<Vec<(ExtractedText, String)>> {
        let (replacements, _, _) = self.process_fields_with_mode(extracted)?;
        Ok(replacements)
    }

    pub fn process_fields(
        &self,
        extracted: &[ExtractedText],
    ) -> anyhow::Result<Vec<(ExtractedText, String)>> {
        self.process_fields_with_mode(extracted)
            .map(|(r, _, _)| r)
    }

    pub fn process_fields_with_mode(
        &self,
        extracted: &[ExtractedText],
    ) -> anyhow::Result<(Vec<(ExtractedText, String)>, u32, u32)> {
        let mut replacements = Vec::new();
        let mut blocks = 0u32;
        let mut observes = 0u32;
        for item in extracted {
            if let Some(matched) = self.check_text(&item.text) {
                let (enforce, rule_id, observe_kind) = match &matched {
                    SecurityMatch::Operation { rule_id, .. } => (
                        self.operation_mode == OperationSecurityMode::Enforce,
                        rule_id.as_str(),
                        "operation security",
                    ),
                    SecurityMatch::PathProtection { rule_id, .. } => (
                        self.path_protection_mode == OperationSecurityMode::Enforce,
                        rule_id.as_str(),
                        "path protection",
                    ),
                };
                if enforce {
                    replacements.push((item.clone(), matched.payload()));
                    blocks += 1;
                } else {
                    observes += 1;
                    tracing::warn!(
                        rule_id = %rule_id,
                        kind = observe_kind,
                        "security observe: policy match detected"
                    );
                }
            }
        }
        Ok((replacements, blocks, observes))
    }

    fn check_text(&self, text: &str) -> Option<SecurityMatch> {
        for compiled in &self.rules {
            if self.matches_rule(text, compiled) {
                let msg = format!(
                    "[SMR BLOCKED] 操作「{:?}: {}」已被安全策略拦截。规则 ID: {}",
                    compiled.rule.operation, compiled.rule.object.pattern, compiled.rule.id
                );
                return Some(SecurityMatch::Operation {
                    payload: wrap_blocked_payload(text, &msg, compiled.rule.operation),
                    rule_id: compiled.rule.id.clone(),
                });
            }
        }

        if let Some((rule_id, level, path)) = self.path_protection.check(text) {
            let msg = format!(
                "[SMR BLOCKED] 路径防护「{}」已拦截对 {} 的操作。规则 ID: {}",
                level_label(level),
                path,
                rule_id
            );
            return Some(SecurityMatch::PathProtection {
                payload: wrap_blocked_payload(text, &msg, OperationType::CommandExec),
                rule_id,
            });
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

impl SecurityMatch {
    fn payload(self) -> String {
        match self {
            Self::Operation { payload, .. } | Self::PathProtection { payload, .. } => payload,
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
    let lower = text.to_lowercase();
    lower.contains("\"function\"")
        || lower.contains("\"tool\"")
        || lower.contains("\"name\":")
        || lower.contains("invoke(")
        || lower.contains("fetch(")
        || lower.contains("grpc")
        || lower.contains("rpc")
        || lower.contains("sdk")
        || lower.contains("runtime.")
        || lower.contains("read_file")
        || lower.contains("write(")
        || ((text.contains("http://") || text.contains("https://"))
            && !lower.contains("curl ")
            && !lower.contains("wget "))
}

fn is_network_access(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("curl ")
        || lower.contains("wget ")
        || lower.contains("web_fetch")
        || lower.contains("http.get")
        || lower.contains("https.get")
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
        let ops = OperationSecurity::new(&rules, &[], OperationSecurityMode::Enforce, OperationSecurityMode::Enforce).unwrap();
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

    #[test]
    fn api_call_matches_tool_invocation_not_shell_curl() {
        let rules = vec![OperationRule {
            id: "block-read".into(),
            enabled: true,
            operation: OperationType::ApiCall,
            object: OperationObject {
                pattern: "read_file".into(),
                is_regex: false,
            },
        }];
        let ops = OperationSecurity::new(&rules, &[], OperationSecurityMode::Enforce, OperationSecurityMode::Enforce).unwrap();
        let extracted = vec![ExtractedText {
            pointer: smr_protocol::TextPointer::OpenAiToolCallArguments {
                message_index: 0,
                tool_index: 0,
            },
            text: r#"{"name":"read_file","arguments":{"path":"/tmp/x"}}"#.into(),
        }];
        let out = ops.process_response(&extracted).unwrap();
        assert_eq!(out.len(), 1);
        assert!(out[0].1.contains("SMR BLOCKED"));
    }

    #[test]
    fn network_access_matches_curl_not_tool_api() {
        let rules = vec![OperationRule {
            id: "block-curl".into(),
            enabled: true,
            operation: OperationType::NetworkAccess,
            object: OperationObject {
                pattern: "https://evil.example".into(),
                is_regex: false,
            },
        }];
        let ops = OperationSecurity::new(&rules, &[], OperationSecurityMode::Enforce, OperationSecurityMode::Enforce).unwrap();
        let extracted = vec![ExtractedText {
            pointer: smr_protocol::TextPointer::OpenAiToolCallArguments {
                message_index: 0,
                tool_index: 0,
            },
            text: r#"{"command":"curl https://evil.example/secret"}"#.into(),
        }];
        let out = ops.process_response(&extracted).unwrap();
        assert_eq!(out.len(), 1);
        assert!(out[0].1.contains("SMR BLOCKED"));
    }

    #[test]
    fn regex_mode_matches_flexible_whitespace() {
        let rules = vec![OperationRule {
            id: "block-rm-flex".into(),
            enabled: true,
            operation: OperationType::CommandExec,
            object: OperationObject {
                pattern: r"(?i)rm\s+-rf".into(),
                is_regex: true,
            },
        }];
        let ops = OperationSecurity::new(&rules, &[], OperationSecurityMode::Enforce, OperationSecurityMode::Enforce).unwrap();
        let extracted = vec![ExtractedText {
            pointer: smr_protocol::TextPointer::OpenAiToolCallArguments {
                message_index: 0,
                tool_index: 0,
            },
            text: r#"{"command":"rm  -rf /"}"#.into(),
        }];
        let out = ops.process_response(&extracted).unwrap();
        assert_eq!(out.len(), 1);
        assert!(out[0].1.contains("SMR BLOCKED"));
    }

    #[test]
    fn literal_mode_does_not_match_extra_whitespace() {
        let rules = vec![OperationRule {
            id: "block-rm".into(),
            enabled: true,
            operation: OperationType::CommandExec,
            object: OperationObject {
                pattern: "rm -rf".into(),
                is_regex: false,
            },
        }];
        let ops = OperationSecurity::new(&rules, &[], OperationSecurityMode::Enforce, OperationSecurityMode::Enforce).unwrap();
        let extracted = vec![ExtractedText {
            pointer: smr_protocol::TextPointer::OpenAiToolCallArguments {
                message_index: 0,
                tool_index: 0,
            },
            text: r#"{"command":"rm  -rf /"}"#.into(),
        }];
        let out = ops.process_response(&extracted).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn path_protection_blocks_via_ops_engine() {
        use crate::config::{PathProtectionLevel, PathProtectionRule};
        use std::path::PathBuf;

        let ops = OperationSecurity::new(
            &[],
            &[PathProtectionRule {
                id: "vault".into(),
                enabled: true,
                path: PathBuf::from("/secure/vault"),
                level: PathProtectionLevel::DenyAccess,
            }],
            OperationSecurityMode::Enforce, OperationSecurityMode::Enforce,
        )
        .unwrap();
        let extracted = vec![ExtractedText {
            pointer: smr_protocol::TextPointer::OpenAiToolCallArguments {
                message_index: 0,
                tool_index: 0,
            },
            text: r#"{"path":"/secure/vault/secret.txt"}"#.into(),
        }];
        let out = ops.process_fields(&extracted).unwrap();
        assert_eq!(out.len(), 1);
        assert!(out[0].1.contains("路径防护"));
    }

    #[test]
    fn path_protection_enforces_while_operation_rules_observe() {
        use crate::config::{PathProtectionLevel, PathProtectionRule};
        use std::path::PathBuf;

        let ops = OperationSecurity::new(
            &[OperationRule {
                id: "block-rm".into(),
                enabled: true,
                operation: OperationType::CommandExec,
                object: OperationObject {
                    pattern: "rm -rf".into(),
                    is_regex: false,
                },
            }],
            &[PathProtectionRule {
                id: "vault".into(),
                enabled: true,
                path: PathBuf::from("/secure/vault"),
                level: PathProtectionLevel::DenyAccess,
            }],
            OperationSecurityMode::Observe,
            OperationSecurityMode::Enforce,
        )
        .unwrap();
        let rm = vec![ExtractedText {
            pointer: smr_protocol::TextPointer::OpenAiToolCallArguments {
                message_index: 0,
                tool_index: 0,
            },
            text: r#"{"command":"rm -rf /"}"#.into(),
        }];
        assert!(ops.process_fields(&rm).unwrap().is_empty());

        let path = vec![ExtractedText {
            pointer: smr_protocol::TextPointer::OpenAiToolCallArguments {
                message_index: 0,
                tool_index: 0,
            },
            text: r#"{"path":"/secure/vault/secret.txt"}"#.into(),
        }];
        let out = ops.process_fields(&path).unwrap();
        assert_eq!(out.len(), 1);
        assert!(out[0].1.contains("路径防护"));
    }
}
