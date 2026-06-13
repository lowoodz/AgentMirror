/// Optional LLM backend for goal refinement and critic enrichment (V1).
pub trait LlmClient: Send + Sync {
    fn complete(&self, system: &str, user: &str) -> anyhow::Result<String>;
}
