use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use parking_lot::RwLock;

use crate::config::AppConfig;
use crate::dlp::{DlpEngine, SessionGuard};
use crate::events::{EventKind, EventLog};
use crate::ops::OperationSecurity;
use crate::router::Router;
use crate::storage::AuditStore;
use crate::paths;
use crate::traffic::TrafficLog;

pub struct AppEngines {
    pub config: AppConfig,
    pub dlp: Arc<DlpEngine>,
    pub ops: Arc<OperationSecurity>,
    pub router: Arc<Router>,
}

impl AppEngines {
    pub fn from_config(config: AppConfig) -> Result<Self> {
        Self::from_config_with_sessions(config, SessionGuard::new())
    }

    pub fn from_config_with_sessions(config: AppConfig, sessions: SessionGuard) -> Result<Self> {
        Self::from_config_with_sessions_and_vault(config, sessions, crate::dlp::TokenVault::new())
    }

    pub fn from_config_with_sessions_and_vault(
        config: AppConfig,
        sessions: SessionGuard,
        vault: crate::dlp::TokenVault,
    ) -> Result<Self> {
        let config_arc = Arc::new(config.clone());
        let ops_enabled = config.pipeline.ops_active();
        Ok(Self {
            dlp: Arc::new(DlpEngine::with_sessions_and_vault(
                &config, sessions, vault,
            )?),
            ops: Arc::new(if ops_enabled {
                OperationSecurity::new(
                    &config.operation_rules,
                    &config.path_protection_rules,
                    config.pipeline.operation_security_mode,
                )?
            } else {
                OperationSecurity::new(&[], &[], config.pipeline.operation_security_mode)?
            }),
            router: Arc::new(Router::new(config_arc)),
            config,
        })
    }

    pub fn from_existing_dlp(config: AppConfig, dlp: Arc<DlpEngine>) -> Result<Self> {
        let config_arc = Arc::new(config.clone());
        let ops_enabled = config.pipeline.ops_active();
        Ok(Self {
            dlp,
            ops: Arc::new(if ops_enabled {
                OperationSecurity::new(
                    &config.operation_rules,
                    &config.path_protection_rules,
                    config.pipeline.operation_security_mode,
                )?
            } else {
                OperationSecurity::new(&[], &[], config.pipeline.operation_security_mode)?
            }),
            router: Arc::new(Router::new(config_arc)),
            config,
        })
    }
}

pub struct SharedApp {
    pub config_path: PathBuf,
    pub events: Arc<EventLog>,
    pub storage: Arc<AuditStore>,
    pub traffic: Arc<TrafficLog>,
    inner: RwLock<AppEngines>,
}

impl SharedApp {
    pub fn new(
        config_path: PathBuf,
        config: AppConfig,
        events: Arc<EventLog>,
        storage: Arc<AuditStore>,
    ) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            config_path,
            events,
            storage,
            traffic: TrafficLog::new(200, paths::traffic_dir()),
            inner: RwLock::new(AppEngines::from_config(config)?),
        }))
    }

    pub fn snapshot(&self) -> EngineSnapshot {
        let g = self.inner.read();
        EngineSnapshot {
            config: g.config.clone(),
            dlp: g.dlp.clone(),
            ops: g.ops.clone(),
            router: g.router.clone(),
        }
    }

    pub fn config(&self) -> AppConfig {
        self.inner.read().config.clone()
    }

    fn replace_engines(&self, config: AppConfig) -> Result<()> {
        let inner = self.inner.read();
        let sessions = inner.dlp.sessions().clone();
        let vault = inner.dlp.vault().clone();
        let file_rules_unchanged = inner.config.file_rules == config.file_rules;
        let reused_dlp = if file_rules_unchanged {
            Some(inner.dlp.clone())
        } else {
            None
        };
        drop(inner);

        let engines = if let Some(dlp) = reused_dlp {
            AppEngines::from_existing_dlp(config.clone(), dlp)?
        } else {
            AppEngines::from_config_with_sessions_and_vault(config.clone(), sessions, vault)?
        };
        *self.inner.write() = engines;
        Ok(())
    }

    pub fn reload(&self) -> Result<()> {
        let config = AppConfig::load(&self.config_path)?;
        self.replace_engines(config)?;
        self.events.push(
            EventKind::ConfigReload,
            format!("reloaded {}", self.config_path.display()),
            None,
        );
        Ok(())
    }

    pub fn save_config(&self, config: &AppConfig) -> Result<()> {
        config.validate()?;
        if let Some(parent) = self.config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let yaml = serde_yaml::to_string(config)?;
        std::fs::write(&self.config_path, yaml)?;
        self.replace_engines(config.clone())?;
        self.events.push(EventKind::ConfigReload, "config saved", None);
        Ok(())
    }

    pub fn load_or_create(config_path: &Path, example_yaml: &str) -> Result<(Arc<Self>, PathBuf)> {
        let events = EventLog::new(500);
        let storage = Arc::new(AuditStore::open(&AuditStore::default_path())?);
        let path = if config_path.as_os_str().is_empty() {
            crate::paths::init_default_config(example_yaml)?
        } else if config_path.exists() {
            config_path.to_path_buf()
        } else if config_path.parent().is_some_and(|p| !p.as_os_str().is_empty()) {
            if let Some(parent) = config_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if !config_path.exists() {
                std::fs::write(config_path, example_yaml)?;
            }
            config_path.to_path_buf()
        } else {
            crate::paths::init_default_config(example_yaml)?
        };

        let config = AppConfig::load(&path)?;
        let app = SharedApp::new(path.clone(), config, events, storage)?;
        app.events.push(
            EventKind::Info,
            format!("started with config {}", path.display()),
            None,
        );
        Ok((app, path))
    }
}

pub struct EngineSnapshot {
    pub config: AppConfig,
    pub dlp: Arc<DlpEngine>,
    pub ops: Arc<OperationSecurity>,
    pub router: Arc<Router>,
}
