use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use smr_protocol::ApiProtocol;

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
    #[serde(default)]
    pub path_protection_rules: Vec<PathProtectionRule>,
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
    #[serde(default = "default_true")]
    pub security_enabled: bool,
    pub dlp_enabled: bool,
    pub operation_security_mode: OperationSecurityMode,
    #[serde(default = "default_true")]
    pub builtin_credential_presets: bool,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            security_enabled: true,
            dlp_enabled: true,
            operation_security_mode: OperationSecurityMode::Observe,
            builtin_credential_presets: true,
        }
    }
}

impl PipelineConfig {
    pub fn dlp_active(&self) -> bool {
        self.security_enabled && self.dlp_enabled
    }

    pub fn ops_active(&self) -> bool {
        self.security_enabled
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
    /// When true, keep recent request/response JSON snapshots in memory for the admin UI.
    #[serde(default)]
    pub save_traffic_bodies: bool,
    /// Max bytes per stored body snapshot (truncated beyond this).
    #[serde(default = "default_traffic_max_body_bytes")]
    pub traffic_max_body_bytes: usize,
}

fn default_traffic_max_body_bytes() -> usize {
    32 * 1024
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            redact_content: true,
            save_traffic_bodies: false,
            traffic_max_body_bytes: default_traffic_max_body_bytes(),
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
    /// Optional explicit API protocol: `openai` or `anthropic`. When omitted, inferred from base_url.
    #[serde(default)]
    pub protocol: Option<String>,
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

    pub fn resolve_protocol(&self) -> ApiProtocol {
        if let Some(protocol) = &self.protocol {
            match protocol.to_ascii_lowercase().as_str() {
                "anthropic" => return ApiProtocol::Anthropic,
                "openai" => return ApiProtocol::OpenAi,
                _ => {}
            }
        }
        let url = self.base_url.to_ascii_lowercase();
        if url.contains("anthropic.com") {
            ApiProtocol::Anthropic
        } else {
            ApiProtocol::OpenAi
        }
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

/// Tunables for disk-backed file index (large corpora, e.g. 10GB+).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FileIndexOptions {
    #[serde(default = "default_index_chunk_size")]
    pub chunk_size: usize,
    #[serde(default = "default_index_chunk_overlap")]
    pub chunk_overlap: usize,
    #[serde(default = "default_signature_stride")]
    pub signature_stride: usize,
    #[serde(default = "default_signatures_per_chunk")]
    pub signatures_per_chunk: usize,
    #[serde(default = "default_max_full_file_bytes")]
    pub max_full_file_bytes: u64,
    #[serde(default = "default_max_haystack_bytes")]
    pub max_haystack_bytes: usize,
    #[serde(default = "default_bloom_megabytes")]
    pub bloom_megabytes: usize,
    #[serde(default = "default_build_workers")]
    pub build_workers: usize,
    #[serde(default = "default_scan_stride")]
    pub scan_stride: usize,
    #[serde(default = "default_scan_workers")]
    pub scan_workers: usize,
    #[serde(default = "default_true")]
    pub scan_rg_prefilter: bool,
    #[serde(default = "default_scan_rg_literals_max")]
    pub scan_rg_literals_max: usize,
}

impl Default for FileIndexOptions {
    fn default() -> Self {
        Self {
            chunk_size: default_index_chunk_size(),
            chunk_overlap: default_index_chunk_overlap(),
            signature_stride: default_signature_stride(),
            signatures_per_chunk: default_signatures_per_chunk(),
            max_full_file_bytes: default_max_full_file_bytes(),
            max_haystack_bytes: default_max_haystack_bytes(),
            bloom_megabytes: default_bloom_megabytes(),
            build_workers: default_build_workers(),
            scan_stride: default_scan_stride(),
            scan_workers: default_scan_workers(),
            scan_rg_prefilter: default_true(),
            scan_rg_literals_max: default_scan_rg_literals_max(),
        }
    }
}

fn default_index_chunk_size() -> usize {
    8192
}
fn default_index_chunk_overlap() -> usize {
    64
}
fn default_signature_stride() -> usize {
    128
}
fn default_signatures_per_chunk() -> usize {
    16
}
fn default_max_full_file_bytes() -> u64 {
    512 * 1024
}
fn default_max_haystack_bytes() -> usize {
    2 * 1024 * 1024
}
fn default_bloom_megabytes() -> usize {
    64
}
fn default_build_workers() -> usize {
    8
}
fn default_scan_stride() -> usize {
    16
}
fn default_scan_workers() -> usize {
    4
}
fn default_scan_rg_literals_max() -> usize {
    2048
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
    #[serde(default)]
    pub index: FileIndexOptions,
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
        "docx".into(),
        "pptx".into(),
        "pdf".into(),
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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PathProtectionRule {
    pub id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub path: PathBuf,
    #[serde(default)]
    pub level: PathProtectionLevel,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PathProtectionLevel {
    /// Block delete/remove operations only.
    DenyDelete,
    /// Block delete and write/modify operations.
    DenyModify,
    /// Block delete, modify, and read/list/access operations.
    #[default]
    DenyAccess,
}

impl AppConfig {
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let config: AppConfig = serde_yaml::from_str(&text)?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.fallback_groups.is_empty() {
            anyhow::bail!("fallback_groups must not be empty");
        }
        if !self
            .fallback_groups
            .contains_key(&self.server.default_fallback_group)
        {
            anyhow::bail!(
                "default_fallback_group '{}' not found in fallback_groups",
                self.server.default_fallback_group
            );
        }
        for (group, endpoints) in &self.fallback_groups {
            if endpoints.is_empty() {
                anyhow::bail!("fallback group '{group}' has no models");
            }
            for ep in endpoints {
                if ep.base_url.is_empty() || ep.model.is_empty() {
                    anyhow::bail!("endpoint '{}' in group '{group}' missing base_url or model", ep.id);
                }
            }
        }
        Ok(())
    }
}
