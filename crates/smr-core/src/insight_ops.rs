use std::sync::Arc;

use smr_insight::SafetyScanner;

use crate::ops::OperationSecurity;

pub struct OpsSafetyScanner(pub Arc<OperationSecurity>);

impl SafetyScanner for OpsSafetyScanner {
    fn scan(&self, text: &str) -> Option<String> {
        self.0.insight_policy_match(text)
    }
}
