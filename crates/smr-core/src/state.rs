use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use parking_lot::RwLock;

use crate::config::{AppConfig, UiLanguage};
use crate::dlp::{DlpEngine, SessionGuard};
use crate::events::{EventKind, EventLog};
use crate::ops::OperationSecurity;
use crate::router::Router;
use crate::storage::AuditStore;
use crate::paths;
use crate::traffic::TrafficLog;
use smr_insight::InsightService;

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
                    config.pipeline.effective_path_protection_mode(),
                    config.server.ui_language,
                )?
            } else {
                OperationSecurity::new(
                    &[],
                    &[],
                    config.pipeline.operation_security_mode,
                    config.pipeline.effective_path_protection_mode(),
                    config.server.ui_language,
                )?
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
                    config.pipeline.effective_path_protection_mode(),
                    config.server.ui_language,
                )?
            } else {
                OperationSecurity::new(
                    &[],
                    &[],
                    config.pipeline.operation_security_mode,
                    config.pipeline.effective_path_protection_mode(),
                    config.server.ui_language,
                )?
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
    pub insight: Arc<InsightService>,
    inner: RwLock<AppEngines>,
}

impl SharedApp {
    pub fn new(
        config_path: PathBuf,
        config: AppConfig,
        events: Arc<EventLog>,
        storage: Arc<AuditStore>,
        insight: Arc<InsightService>,
    ) -> Result<Arc<Self>> {
        let app = Arc::new(Self {
            config_path,
            events,
            storage,
            traffic: TrafficLog::from_logging_config(&config.logging, paths::traffic_dir()),
            insight,
            inner: RwLock::new(AppEngines::from_config(config)?),
        });
        app.sync_insight_safety();
        Ok(app)
    }

    fn sync_insight_safety(&self) {
        let inner = self.inner.read();
        let ops = inner.ops.clone();
        let router = inner.router.clone();
        let cfg = inner.config.insight.clone();
        drop(inner);
        self.insight
            .set_safety_scanner(Some(Arc::new(crate::insight_ops::OpsSafetyScanner(ops))));
        if cfg.llm_critic || cfg.llm_daily {
            self.insight.set_llm_client(Some(Arc::new(
                crate::insight_llm::RouterLlmClient::new(router.clone(), &cfg.critic_model_group),
            )));
            crate::insight_llm::ensure_background_critic_probe(
                router,
                cfg.critic_model_group.clone(),
            );
        } else {
            self.insight.set_llm_client(None);
        }
    }

    fn sync_insight_report_language(config: &mut AppConfig) {
        config.insight.report_language = match config.server.ui_language {
            UiLanguage::Zh => "zh".to_string(),
            UiLanguage::En => "en".to_string(),
        };
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
            dlp.reload(&config)?;
            AppEngines::from_existing_dlp(config.clone(), dlp)?
        } else {
            AppEngines::from_config_with_sessions_and_vault(config.clone(), sessions, vault)?
        };
        engines.ops.sync_runtime_config(config.server.ui_language);
        *self.inner.write() = engines;
        self.sync_insight_safety();
        Ok(())
    }

    pub fn reload(&self) -> Result<()> {
        let mut config = AppConfig::load(&self.config_path)?;
        Self::sync_insight_report_language(&mut config);
        self.traffic.apply_policy(&config.logging);
        self.insight.apply_config(&config.insight);
        self.replace_engines(config)?;
        self.events.push(
            EventKind::ConfigReload,
            format!("reloaded {}", self.config_path.display()),
            None,
        );
        Ok(())
    }

    pub fn save_config(&self, config: &AppConfig) -> Result<()> {
        let mut config = config.clone();
        config.pipeline.normalize_modes();
        config.logging.normalize_traffic_policy();
        config.insight.normalize();
        Self::sync_insight_report_language(&mut config);
        if config.insight.enabled && config.insight.require_traffic_bodies {
            config.logging.save_traffic_bodies = true;
        }
        config.validate()?;
        if let Some(parent) = self.config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let yaml = serde_yaml::to_string(&config)?;
        std::fs::write(&self.config_path, yaml)?;
        self.traffic.apply_policy(&config.logging);
        self.insight.apply_config(&config.insight);
        self.replace_engines(config)?;
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

        let mut config = AppConfig::load(&path)?;
        SharedApp::sync_insight_report_language(&mut config);
        let insight = InsightService::open(
            &crate::paths::data_dir(),
            crate::paths::insight_graphs_dir(),
            config.insight.clone(),
        )?;
        let app = SharedApp::new(path.clone(), config, events, storage, insight)?;
        app.events.push(
            EventKind::Info,
            format!("started with config {}", path.display()),
            None,
        );
        Ok((app, path))
    }

    pub fn replay_from_traffic(
        &self,
        limit: usize,
    ) -> anyhow::Result<crate::insight_replay::ReplayStats> {
        crate::insight_replay::replay_from_traffic(self, limit)
    }

    pub fn reset_insight(&self) -> anyhow::Result<smr_insight::ResetStats> {
        crate::insight_replay::reset_insight(self)
    }
}

pub struct EngineSnapshot {
    pub config: AppConfig,
    pub dlp: Arc<DlpEngine>,
    pub ops: Arc<OperationSecurity>,
    pub router: Arc<Router>,
}
