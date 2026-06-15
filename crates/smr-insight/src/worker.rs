use std::path::Path;
use std::sync::Arc;
use std::thread;

use chrono::{Local, NaiveDate, Timelike};
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

use crate::llm::LlmClient;
use crate::models::{InsightConfig, TraceTurn};
use crate::pipeline::Pipeline;
use crate::report::{daily_report_markdown, generate_all_agents_daily_report, sweep_idle_running_runs};
use crate::safety::SafetyScanner;
use crate::store::InsightStore;

const QUEUE_CAPACITY: usize = 256;

type SafetySlot = Arc<Mutex<Option<Arc<dyn SafetyScanner>>>>;
type LlmSlot = Arc<Mutex<Option<Arc<dyn LlmClient>>>>;

pub struct InsightService {
    config: Arc<parking_lot::RwLock<InsightConfig>>,
    tx: mpsc::Sender<TraceTurn>,
    store: Arc<InsightStore>,
    safety: SafetySlot,
    llm: LlmSlot,
}

impl InsightService {
    pub fn open(
        data_dir: &Path,
        graphs_dir: std::path::PathBuf,
        config: InsightConfig,
    ) -> anyhow::Result<Arc<Self>> {
        let store = Arc::new(InsightStore::open(data_dir, graphs_dir)?);
        let (tx, rx) = mpsc::channel(QUEUE_CAPACITY);
        let config_slot = Arc::new(parking_lot::RwLock::new(config.clone()));

        if config.retention_days > 0 {
            if let Err(err) = store.purge_older_than(config.retention_days) {
                warn!(?err, "AgentMirror retention purge on startup failed");
            }
        }

        let safety: SafetySlot = Arc::new(Mutex::new(None));
        let llm: LlmSlot = Arc::new(Mutex::new(None));
        spawn_worker(
            rx,
            Arc::clone(&store),
            Arc::clone(&safety),
            Arc::clone(&llm),
            Arc::clone(&config_slot),
        );
        spawn_daily_scheduler(
            Arc::clone(&store),
            Arc::clone(&llm),
            Arc::clone(&config_slot),
        );
        spawn_idle_run_sweeper(
            Arc::clone(&store),
            Arc::clone(&safety),
            Arc::clone(&llm),
            Arc::clone(&config_slot),
        );

        Ok(Arc::new(Self {
            config: config_slot,
            tx,
            store,
            safety,
            llm,
        }))
    }

    pub fn set_safety_scanner(&self, scanner: Option<Arc<dyn SafetyScanner>>) {
        *self.safety.lock() = scanner;
    }

    pub fn set_llm_client(&self, client: Option<Arc<dyn LlmClient>>) {
        *self.llm.lock() = client;
    }

    pub fn apply_config(&self, config: &InsightConfig) {
        *self.config.write() = config.clone();
    }

    pub fn config(&self) -> InsightConfig {
        self.config.read().clone()
    }

    pub fn store(&self) -> Arc<InsightStore> {
        Arc::clone(&self.store)
    }

    pub fn enabled(&self) -> bool {
        self.config.read().enabled
    }

    pub fn submit_turn(&self, turn: TraceTurn) {
        if !self.enabled() {
            return;
        }
        if turn.request_body.is_empty() && turn.response_body.is_empty() {
            return;
        }
        match self.tx.try_send(turn) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                warn!("AgentMirror queue full — dropping trace turn");
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                warn!("AgentMirror worker channel closed");
            }
        }
    }

    pub fn generate_daily_for_date(&self, date: NaiveDate) -> anyhow::Result<usize> {
        let cfg = self.config.read().clone();
        let llm = self.llm.lock().clone();
        let daily_dir = self.store.daily_reports_dir();
        std::fs::create_dir_all(&daily_dir)?;
        if let Some(report) = generate_all_agents_daily_report(
            &self.store,
            date,
            llm.as_deref(),
            cfg.llm_daily,
            cfg.report_language(),
        )? {
            self.store.save_daily_report(&report)?;
            let md = daily_report_markdown(&report);
            let path = daily_dir.join(format!("{}_all.md", report.date));
            let _ = std::fs::write(path, md);
            Ok(1)
        } else {
            Ok(0)
        }
    }

    pub fn reset(&self) -> anyhow::Result<crate::store::ResetStats> {
        self.store.reset_all()
    }

    /// Process one turn on the calling thread (for traffic replay; bypasses the async queue).
    pub fn process_turn_sync(&self, turn: TraceTurn) -> anyhow::Result<()> {
        let store = Arc::clone(&self.store);
        let scanner = self.safety.lock().clone();
        let llm_client = self.llm.lock().clone();
        let cfg = self.config.read().clone();
        Pipeline::new(store, scanner, llm_client, cfg).process_turn(turn)
    }

    /// After traffic replay: complete open runs and generate LLM reflection reports.
    pub fn finalize_replayed_runs(&self) -> anyhow::Result<usize> {
        let cfg = self.config.read().clone();
        let scanner = self.safety.lock().clone();
        let llm = self.llm.lock().clone();
        crate::report::finalize_runs_for_llm_reports(
            &self.store,
            scanner.as_deref(),
            llm.as_deref(),
            cfg.llm_critic,
            cfg.report_language(),
        )
    }
}

/// Own Tokio runtime on a background thread so InsightService works from Tauri/GUI
/// startup (no reactor on the main thread yet).
fn spawn_worker(
    mut rx: mpsc::Receiver<TraceTurn>,
    store: Arc<InsightStore>,
    safety: SafetySlot,
    llm: LlmSlot,
    config: Arc<parking_lot::RwLock<InsightConfig>>,
) {
    thread::Builder::new()
        .name("agentmirror-worker".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("AgentMirror worker runtime");
            rt.block_on(async move {
                while let Some(turn) = rx.recv().await {
                    let audit_id = turn.audit_id.clone();
                    let store = Arc::clone(&store);
                    let scanner = safety.lock().clone();
                    let llm_client = llm.lock().clone();
                    let cfg = config.read().clone();
                    match Pipeline::new(store, scanner, llm_client, cfg).process_turn(turn) {
                        Ok(()) => debug!(audit_id = %audit_id, "AgentMirror processed turn"),
                        Err(err) => error!(?err, audit_id = %audit_id, "AgentMirror process error"),
                    }
                }
            });
        })
        .expect("spawn AgentMirror worker thread");
}

fn spawn_daily_scheduler(
    store: Arc<InsightStore>,
    llm: LlmSlot,
    config: Arc<parking_lot::RwLock<InsightConfig>>,
) {
    thread::Builder::new()
        .name("agentmirror-daily".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("AgentMirror daily runtime");
            rt.block_on(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                    let cfg = config.read().clone();
                    let hour = cfg.daily_report_hour;
                    if Local::now().hour() != u32::from(hour) {
                        continue;
                    }
                    let yesterday = (Local::now() - chrono::Duration::days(1)).date_naive();
                    let llm_client = llm.lock().clone();
                    if let Ok(Some(report)) = generate_all_agents_daily_report(
                        &store,
                        yesterday,
                        llm_client.as_deref(),
                        cfg.llm_daily,
                        cfg.report_language(),
                    ) {
                        let _ = store.save_daily_report(&report);
                    }
                    let retention = cfg.retention_days;
                    if retention > 0 {
                        if let Err(err) = store.purge_older_than(retention) {
                            warn!(?err, "AgentMirror scheduled retention purge failed");
                        }
                    }
                }
            });
        })
        .expect("spawn AgentMirror daily scheduler thread");
}

fn spawn_idle_run_sweeper(
    store: Arc<InsightStore>,
    safety: SafetySlot,
    llm: LlmSlot,
    config: Arc<parking_lot::RwLock<InsightConfig>>,
) {
    thread::Builder::new()
        .name("agentmirror-idle".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("AgentMirror idle sweeper runtime");
            rt.block_on(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    if !config.read().enabled || !config.read().llm_critic {
                        continue;
                    }
                    let scanner = safety.lock().clone();
                    let llm_client = llm.lock().clone();
                    let llm_critic = config.read().llm_critic;
                    let language = config.read().report_language();
                    if let Err(err) = sweep_idle_running_runs(
                        &store,
                        scanner.as_deref(),
                        llm_client.as_deref(),
                        llm_critic,
                        language,
                    ) {
                        warn!(?err, "AgentMirror idle run sweep failed");
                    }
                }
            });
        })
        .expect("spawn AgentMirror idle sweeper thread");
}
