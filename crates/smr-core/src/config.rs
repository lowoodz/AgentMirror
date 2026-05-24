use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub pipeline: PipelineConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub fallback_groups: HashMap<String, Vec<ModelEndpoint>>,
    #[serde(default)]
    pub content_rules: Vec<ContentRule>,
    #[serde(default)]
    pub file_rules: Vec<FileRule>,
    #[serde(default)]
    pub operation_rules: Vec<OperationRule>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerConfig {
    pub listen: String,
    pub default_fallback_group: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:8080".to_string(),
            default_fallback_group: "high".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PipelineConfig {
    pub dlp_enabled: bool,
    pub operation_security_mode: OperationSecurityMode,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            dlp_enabled: true,
            operation_security_mode: OperationSecurityMode::Observe,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OperationSecurityMode {
    Observe,
    Enforce,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LoggingConfig {
    pub level: String,
    pub redact_content: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            redact_content: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelEndpoint {
    pub id: String,
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_timeout_secs() -> u64 {
    120
}

impl ModelEndpoint {
    pub fn resolve_api_key(&self) -> Option<String> {
        if let Some(key) = &self.api_key {
            if !key.is_empty() {
                return Some(key.clone());
            }
        }
        if let Some(env_name) = &self.api_key_env {
            return std::env::var(env_name).ok();
        }
        None
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ContentRule {
    pub id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub match_mode: MatchMode,
    pub value: String,
    #[serde(default)]
    pub category: ContentCategory,
    #[serde(default)]
    pub min_fragment_len: Option<usize>,
    #[serde(default)]
    pub min_fragment_ratio: Option<f64>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum MatchMode {
    Full,
    #[default]
    Fragment,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ContentCategory {
    #[default]
    Normal,
    Secret,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FileRule {
    pub id: String,
    pub path: PathBuf,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub recursive: bool,
    #[serde(default = "default_trigger_window")]
    pub trigger_window: u32,
    #[serde(default)]
    pub match_mode: MatchMode,
    #[serde(default)]
    pub min_fragment_len: Option<usize>,
    #[serde(default)]
    pub min_fragment_ratio: Option<f64>,
    #[serde(default = "default_formats")]
    pub formats: Vec<String>,
}

fn default_true() -> bool {
    true
}

fn default_trigger_window() -> u32 {
    5
}

fn default_formats() -> Vec<String> {
    vec![
        "txt".into(),
        "md".into(),
        "json".into(),
        "yaml".into(),
        "yml".into(),
        "rs".into(),
        "py".into(),
        "js".into(),
        "ts".into(),
        "tsx".into(),
        "jsx".into(),
        "html".into(),
        "css".into(),
        "xml".into(),
        "csv".into(),
    ]
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OperationRule {
    pub id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub operation: OperationType,
    pub object: OperationObject,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperationType {
    CommandExec,
    ApiCall,
    NetworkAccess,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OperationObject {
    pub pattern: String,
    #[serde(default)]
    pub is_regex: bool,
}

impl AppConfig {
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let config: AppConfig = serde_yaml::from_str(&text)?;
        Ok(config)
    }
}
