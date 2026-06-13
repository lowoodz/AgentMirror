/// Optional hook for SafeRoute operation/path rules to score AgentMirror safety critic.
pub trait SafetyScanner: Send + Sync {
    /// Returns a short human-readable finding when `text` matches a security policy.
    fn scan(&self, text: &str) -> Option<String>;
}

pub fn scan_action_events(
    events: &[crate::models::CognitiveEvent],
    scanner: Option<&dyn SafetyScanner>,
) -> Vec<String> {
    let Some(scanner) = scanner else {
        return Vec::new();
    };
    let mut findings = Vec::new();
    for event in events {
        if event.kind != crate::models::EventKind::Action {
            continue;
        }
        if let Some(msg) = scanner.scan(&event.summary) {
            if !findings.iter().any(|f| f == &msg) {
                findings.push(msg);
            }
        }
    }
    findings
}
