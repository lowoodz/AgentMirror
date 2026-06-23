use dashmap::DashMap;
use parking_lot::RwLock;
use regex::Regex;

use crate::config::{
    OperationRule, OperationSecurityMode, OperationType, PathProtectionRule, UiLanguage,
};
use smr_protocol::ExtractedText;

mod path_protection;
use path_protection::PathProtection;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OpsBlockKinds {
    pub path: bool,
    pub operation: bool,
}

impl OpsBlockKinds {
    pub fn is_empty(self) -> bool {
        !self.path && !self.operation
    }

    pub fn merge(&mut self, other: Self) {
        self.path |= other.path;
        self.operation |= other.operation;
    }
}

pub struct OperationSecurity {
    rules: Vec<CompiledRule>,
    path_protection: PathProtection,
    operation_mode: OperationSecurityMode,
    path_protection_mode: OperationSecurityMode,
    ui_language: RwLock<UiLanguage>,
    /// Response-side blocks queue system notices for the next client request.
    pending_notices: DashMap<String, OpsBlockKinds>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Operation,
    PathProtection,
}

impl OperationSecurity {
    pub fn new(
        rules: &[OperationRule],
        path_rules: &[PathProtectionRule],
        operation_mode: OperationSecurityMode,
        path_protection_mode: OperationSecurityMode,
        ui_language: UiLanguage,
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
            ui_language: RwLock::new(ui_language),
            pending_notices: DashMap::new(),
        })
    }

    pub fn sync_runtime_config(&self, ui_language: UiLanguage) {
        *self.ui_language.write() = ui_language;
    }

    pub fn ui_language(&self) -> UiLanguage {
        *self.ui_language.read()
    }

    pub fn mark_pending_notices(&self, session_id: &str, kinds: OpsBlockKinds) {
        if kinds.is_empty() {
            return;
        }
        self.pending_notices
            .entry(session_id.to_string())
            .and_modify(|existing| existing.merge(kinds))
            .or_insert(kinds);
    }

    pub fn take_pending_notices(&self, session_id: &str) -> OpsBlockKinds {
        self.pending_notices
            .remove(session_id)
            .map(|(_, kinds)| kinds)
            .unwrap_or_default()
    }

    pub fn notice_text_for_kinds(&self, kinds: OpsBlockKinds) -> Option<String> {
        let lang = self.ui_language();
        let mut parts = Vec::new();
        if kinds.path {
            parts.push(lang.path_protection_system_notice());
        }
        if kinds.operation {
            parts.push(lang.operation_security_system_notice());
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n"))
        }
    }

    pub fn process_response(
        &self,
        extracted: &[ExtractedText],
    ) -> anyhow::Result<Vec<(ExtractedText, String)>> {
        let (replacements, _, _, _) = self.process_fields_with_mode(extracted)?;
        Ok(replacements)
    }

    pub fn process_fields(
        &self,
        extracted: &[ExtractedText],
    ) -> anyhow::Result<Vec<(ExtractedText, String)>> {
        self.process_fields_with_mode(extracted)
            .map(|(r, _, _, _)| r)
    }

    pub fn process_fields_with_mode(
        &self,
        extracted: &[ExtractedText],
    ) -> anyhow::Result<(Vec<(ExtractedText, String)>, u32, u32, OpsBlockKinds)> {
        let mut replacements = Vec::new();
        let mut blocks = 0u32;
        let mut observes = 0u32;
        let mut kinds = OpsBlockKinds::default();
        for item in extracted {
            if let Some((payload, enforced, kind)) = self.check_and_enforce(&item.text) {
                if enforced {
                    replacements.push((item.clone(), payload));
                    blocks += 1;
                    match kind {
                        BlockKind::Operation => kinds.operation = true,
                        BlockKind::PathProtection => kinds.path = true,
                    }
                } else {
                    observes += 1;
                }
            }
        }
        Ok((replacements, blocks, observes, kinds))
    }

    /// Assembled streaming tool-call arguments (e.g. exec JSON). Returns blocked payload when enforce triggers.
    pub fn enforce_tool_call(&self, arguments: &str) -> Option<(String, OpsBlockKinds)> {
        self.check_and_enforce(arguments)
            .filter(|(_, enforced, _)| *enforced)
            .map(|(payload, _, kind)| {
                let mut kinds = OpsBlockKinds::default();
                match kind {
                    BlockKind::Operation => kinds.operation = true,
                    BlockKind::PathProtection => kinds.path = true,
                }
                (payload, kinds)
            })
    }

    /// Returns a short finding for AgentMirror when text matches any enabled policy.
    pub fn insight_policy_match(&self, text: &str) -> Option<String> {
        let matched = self.check_text(text)?;
        let lang = self.ui_language();
        Some(match matched {
            SecurityMatch::Operation { rule_id, .. } => {
                lang.insight_operation_rule_match(&rule_id)
            }
            SecurityMatch::PathProtection { rule_id, .. } => {
                lang.insight_path_protection_rule_match(&rule_id)
            }
        })
    }

    fn check_and_enforce(&self, text: &str) -> Option<(String, bool, BlockKind)> {
        let matched = self.check_text(text)?;
        let block_kind = matched.block_kind();
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
            Some((matched.payload(), true, block_kind))
        } else {
            tracing::warn!(
                rule_id = %rule_id,
                kind = observe_kind,
                "security observe: policy match detected"
            );
            Some((matched.payload(), false, block_kind))
        }
    }

    fn check_text(&self, text: &str) -> Option<SecurityMatch> {
        let lang = self.ui_language();
        for compiled in &self.rules {
            if self.matches_rule(text, compiled) {
                let msg = lang.operation_block_message(
                    compiled.rule.operation,
                    &compiled.rule.object.pattern,
                    &compiled.rule.id,
                );
                return Some(SecurityMatch::Operation {
                    payload: wrap_blocked_payload(text, &msg, BlockKind::Operation),
                    rule_id: compiled.rule.id.clone(),
                });
            }
        }

        if let Some((rule_id, level, path)) = self.path_protection.check(text) {
            let msg = lang.path_protection_block_message(level, &path, &rule_id);
            return Some(SecurityMatch::PathProtection {
                payload: wrap_blocked_payload(text, &msg, BlockKind::PathProtection),
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

    fn block_kind(&self) -> BlockKind {
        match self {
            Self::Operation { .. } => BlockKind::Operation,
            Self::PathProtection { .. } => BlockKind::PathProtection,
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
        || lower.contains("rm -f")
        || lower.contains("rm -r")
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

fn wrap_blocked_payload(original: &str, message: &str, kind: BlockKind) -> String {
    if !original.trim_start().starts_with('{') {
        return message.to_string();
    }

    let smr_block_kind = match kind {
        BlockKind::Operation => "operation_security",
        BlockKind::PathProtection => "path_protection",
    };
    // OpenClaw `exec` requires a `command` field — use a shell no-op plus the block text.
    let command = format!(": # {message}");

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(original) {
        if let Some(obj) = value.as_object() {
            let cmd_key = if obj.contains_key("command") {
                "command"
            } else if obj.contains_key("cmd") {
                "cmd"
            } else {
                "command"
            };
            return serde_json::json!({
                "smr_blocked": true,
                "smr_block_kind": smr_block_kind,
                cmd_key: command,
                "message": message,
            })
            .to_string();
        }
    }

    serde_json::json!({
        "smr_blocked": true,
        "smr_block_kind": smr_block_kind,
        "command": command,
        "message": message,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{OperationObject, OperationRule, OperationType, UiLanguage};

    fn test_ops(
        rules: &[OperationRule],
        path_rules: &[PathProtectionRule],
        operation_mode: OperationSecurityMode,
        path_protection_mode: OperationSecurityMode,
    ) -> OperationSecurity {
        OperationSecurity::new(
            rules,
            path_rules,
            operation_mode,
            path_protection_mode,
            UiLanguage::Zh,
        )
        .unwrap()
    }

    #[test]
    fn operation_block_message_english() {
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
            &[],
            OperationSecurityMode::Enforce,
            OperationSecurityMode::Enforce,
            UiLanguage::En,
        )
        .unwrap();
        let extracted = vec![ExtractedText {
            pointer: smr_protocol::TextPointer::OpenAiToolCallArguments {
                message_index: 0,
                tool_index: 0,
            },
            text: r#"{"command":"rm -rf /"}"#.into(),
        }];
        let out = ops.process_response(&extracted).unwrap();
        assert_eq!(out.len(), 1);
        assert!(out[0].1.contains("security policy"));
        assert!(out[0].1.contains("command execution"));
    }

    #[test]
    fn path_protection_blocked_exec_preserves_command_schema() {
        use crate::config::{PathProtectionLevel, PathProtectionRule};
        use std::path::PathBuf;

        let ops = test_ops(
            &[],
            &[PathProtectionRule {
                id: "vault".into(),
                enabled: true,
                path: PathBuf::from("/secure/vault"),
                level: PathProtectionLevel::DenyAccess,
            }],
            OperationSecurityMode::Enforce,
            OperationSecurityMode::Enforce,
        );
        let blocked = ops
            .enforce_tool_call(r#"{"command":"ls /secure/vault"}"#)
            .expect("blocked")
            .0;
        let parsed: serde_json::Value = serde_json::from_str(&blocked).unwrap();
        assert_eq!(parsed["smr_block_kind"], "path_protection");
        assert!(parsed.get("command").and_then(|v| v.as_str()).is_some());
        assert!(blocked.contains("SMR BLOCKED"));
        assert!(blocked.contains("路径防护"));
    }

    #[test]
    fn pending_notices_merge_and_take() {
        use crate::config::{PathProtectionLevel, PathProtectionRule};
        use std::path::PathBuf;

        let ops = test_ops(
            &[],
            &[PathProtectionRule {
                id: "vault".into(),
                enabled: true,
                path: PathBuf::from("/secure/vault"),
                level: PathProtectionLevel::DenyAccess,
            }],
            OperationSecurityMode::Enforce,
            OperationSecurityMode::Enforce,
        );
        let mut path_only = OpsBlockKinds::default();
        path_only.path = true;
        ops.mark_pending_notices("sess-1", path_only);
        let mut op_only = OpsBlockKinds::default();
        op_only.operation = true;
        ops.mark_pending_notices("sess-1", op_only);
        let taken = ops.take_pending_notices("sess-1");
        assert!(taken.path);
        assert!(taken.operation);
        assert!(ops.take_pending_notices("sess-1").is_empty());
    }

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
        let ops = test_ops(&rules, &[], OperationSecurityMode::Enforce, OperationSecurityMode::Enforce);
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
        let ops = test_ops(&rules, &[], OperationSecurityMode::Enforce, OperationSecurityMode::Enforce);
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
        let ops = test_ops(&rules, &[], OperationSecurityMode::Enforce, OperationSecurityMode::Enforce);
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
        let ops = test_ops(&rules, &[], OperationSecurityMode::Enforce, OperationSecurityMode::Enforce);
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
        let ops = test_ops(&rules, &[], OperationSecurityMode::Enforce, OperationSecurityMode::Enforce);
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

        let ops = test_ops(
            &[],
            &[PathProtectionRule {
                id: "vault".into(),
                enabled: true,
                path: PathBuf::from("/secure/vault"),
                level: PathProtectionLevel::DenyAccess,
            }],
            OperationSecurityMode::Enforce,
            OperationSecurityMode::Enforce,
        );
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

        let ops = test_ops(
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
        );
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

    #[test]
    fn blocks_user_message_with_protected_path() {
        use crate::config::{PathProtectionLevel, PathProtectionRule};
        use std::path::PathBuf;

        let ops = test_ops(
            &[],
            &[PathProtectionRule {
                id: "vault".into(),
                enabled: true,
                path: PathBuf::from("/data/vault"),
                level: PathProtectionLevel::DenyAccess,
            }],
            OperationSecurityMode::Enforce,
            OperationSecurityMode::Enforce,
        );
        let body = serde_json::json!({
            "messages": [
                {"role": "user", "content": "ls /data/vault/secret.md 查看大小"}
            ]
        });
        let extracted = smr_protocol::extract_texts(&body).unwrap();
        let fields = smr_protocol::filter_ops_request_fields(&body, &extracted);
        let out = ops.process_fields(&fields).unwrap();
        assert_eq!(out.len(), 1);
        assert!(out[0].1.contains("路径防护"));
    }

    #[test]
    fn blocks_remove_item_when_operation_rule_enabled() {
        let ops = test_ops(
            &[OperationRule {
                id: "block-remove-item".into(),
                enabled: true,
                operation: OperationType::CommandExec,
                object: OperationObject {
                    pattern: "(?i)remove-item".into(),
                    is_regex: true,
                },
            }],
            &[],
            OperationSecurityMode::Enforce,
            OperationSecurityMode::Enforce,
        );
        let cmd = r#"{"command":"Remove-Item -Path D:\\docs\\hello.txt -Force"}"#;
        let blocked = ops.enforce_tool_call(cmd).expect("should block");
        assert!(blocked.0.contains("SMR BLOCKED"));
    }

    #[test]
    fn disabled_operation_rules_do_not_block() {
        let ops = test_ops(
            &[OperationRule {
                id: "block-remove-item".into(),
                enabled: false,
                operation: OperationType::CommandExec,
                object: OperationObject {
                    pattern: "(?i)remove-item".into(),
                    is_regex: true,
                },
            }],
            &[],
            OperationSecurityMode::Enforce,
            OperationSecurityMode::Enforce,
        );
        let cmd = r#"{"command":"Remove-Item -Path D:\\docs\\hello.txt -Force"}"#;
        assert!(
            ops.enforce_tool_call(cmd).is_none(),
            "disabled rules must not block even when pattern matches"
        );
    }
}
