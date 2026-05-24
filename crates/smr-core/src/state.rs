use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use parking_lot::RwLock;

use crate::config::AppConfig;
use crate::dlp::DlpEngine;
use crate::events::{EventKind, EventLog};
use crate::ops::OperationSecurity;
use crate::router::Router;

pub struct AppEngines {
    pub config: AppConfig,
    pub dlp: Arc<DlpEngine>,
    pub ops: Arc<OperationSecurity>,
    pub router: Arc<Router>,
}

impl AppEngines {
    pub fn from_config(config: AppConfig) -> Result<Self> {
        let config_arc = Arc::new(config.clone());
        Ok(Self {
            dlp: Arc::new(DlpEngine::new(&config)?),
            ops: Arc::new(OperationSecurity::new(
                &config.operation_rules,
                config.pipeline.operation_security_mode,
            )?),
            router: Arc::new(Router::new(config_arc)),
            config,
        })
    }
}

pub struct SharedApp {
    pub config_path: PathBuf,
    pub events: Arc<EventLog>,
    inner: RwLock<AppEngines>,
}

impl SharedApp {
    pub fn new(config_path: PathBuf, config: AppConfig, events: Arc<EventLog>) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            config_path,
            events,
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

    pub fn reload(&self) -> Result<()> {
        let config = AppConfig::load(&self.config_path)?;
        *self.inner.write() = AppEngines::from_config(config)?;
        self.events.push(
            EventKind::ConfigReload,
            format!("reloaded {}", self.config_path.display()),
            None,
        );
        Ok(())
    }

    pub fn save_config(&self, config: &AppConfig) -> Result<()> {
        if let Some(parent) = self.config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let yaml = serde_yaml::to_string(config)?;
        std::fs::write(&self.config_path, yaml)?;
        *self.inner.write() = AppEngines::from_config(config.clone())?;
        self.events.push(EventKind::ConfigReload, "config saved", None);
        Ok(())
    }

    pub fn load_or_create(config_path: &Path, example_yaml: &str) -> Result<(Arc<Self>, PathBuf)> {
        let events = EventLog::new(500);
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
        let app = SharedApp::new(path.clone(), config, events)?;
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
