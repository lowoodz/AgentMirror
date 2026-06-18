use std::path::Path;
use std::sync::Arc;
use std::thread;

use chrono::{Local, NaiveDate, Timelike};
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

use crate::llm::LlmClient;
use crate::metrics::{InsightMetrics, InsightMetricsSnapshot, SharedMetrics};
use crate::models::{InsightConfig, TraceTurn};
use crate::pipeline::Pipeline;
use crate::report::{
    daily_report_markdown, generate_llm_reflection_for_run, sweep_idle_running_runs,
    try_generate_daily_report, DailyGenerateOutcome,
};
use crate::safety::SafetyScanner;
use crate::spill::SpillQueue;
use crate::store::InsightStore;

const QUEUE_CAPACITY: usize = 256;
const LLM_CRITIC_QUEUE_CAPACITY: usize = 32;
pub const META_LAST_DAILY_REPORT_DATE: &str = "last_daily_report_date";

type SafetySlot = Arc<Mutex<Option<Arc<dyn SafetyScanner>>>>;
type LlmSlot = Arc<Mutex<Option<Arc<dyn LlmClient>>>>;

pub struct InsightService {
    config: Arc<parking_lot::RwLock<InsightConfig>>,
    tx: mpsc::Sender<TraceTurn>,
    critic_tx: mpsc::Sender<String>,
    store: Arc<InsightStore>,
    safety: SafetySlot,
    llm: LlmSlot,
    metrics: SharedMetrics,
    spill: Arc<SpillQueue>,
}

impl InsightService {
    pub fn open(
        data_dir: &Path,
        graphs_dir: std::path::PathBuf,
        config: InsightConfig,
    ) -> anyhow::Result<Arc<Self>> {
        let store = Arc::new(InsightStore::open(data_dir, graphs_dir)?);
        let spill = Arc::new(SpillQueue::open(data_dir)?);
        let metrics = Arc::new(InsightMetrics::default());
        let (tx, rx) = mpsc::channel(QUEUE_CAPACITY);
        let (critic_tx, critic_rx) = mpsc::channel(LLM_CRITIC_QUEUE_CAPACITY);
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
            critic_tx.clone(),
            Arc::clone(&store),
            Arc::clone(&spill),
            Arc::clone(&metrics),
            Arc::clone(&safety),
            Arc::clone(&config_slot),
        );
        spawn_llm_critic_worker(
            critic_rx,
            Arc::clone(&store),
            Arc::clone(&safety),
            Arc::clone(&llm),
            Arc::clone(&metrics),
            Arc::clone(&config_slot),
        );
        spawn_daily_scheduler(
            Arc::clone(&store),
            Arc::clone(&llm),
            Arc::clone(&metrics),
            Arc::clone(&config_slot),
        );
        spawn_idle_run_sweeper(
            Arc::clone(&store),
            critic_tx.clone(),
            Arc::clone(&config_slot),
        );

        Ok(Arc::new(Self {
            config: config_slot,
            tx,
            critic_tx,
            store,
            safety,
            llm,
            metrics,
            spill,
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

    pub fn metrics_snapshot(&self) -> InsightMetricsSnapshot {
        self.metrics.snapshot(
            self.spill.pending_count(),
            self.spill.oldest_pending_age_secs(),
        )
    }

    pub fn submit_turn(&self, turn: TraceTurn) {
        if !self.enabled() {
            return;
        }
        if turn.request_body.is_empty() && turn.response_body.is_empty() {
            return;
        }
        match self.tx.try_send(turn) {
            Ok(()) => {
                self.metrics
                    .queue_depth
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            Err(mpsc::error::TrySendError::Full(turn)) => {
                self.metrics.dropped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                match self.spill.push(&turn) {
                    Ok(()) => {
                        self.metrics
                            .spilled
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let dropped = self.metrics.dropped.load(std::sync::atomic::Ordering::Relaxed);
                        let pending = self.spill.pending_count();
                        warn!(
                            dropped,
                            spill_pending = pending,
                            audit_id = %turn.audit_id,
                            "AgentMirror queue full — spilled trace turn to disk"
                        );
                    }
                    Err(err) => {
                        let dropped = self.metrics.dropped.load(std::sync::atomic::Ordering::Relaxed);
                        error!(
                            ?err,
                            dropped,
                            audit_id = %turn.audit_id,
                            "AgentMirror queue full — dropped trace turn (spill failed)"
                        );
                    }
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                warn!("AgentMirror worker channel closed");
            }
        }
    }

    fn enqueue_llm_report(&self, run_id: &str) {
        match self.critic_tx.try_send(run_id.to_string()) {
            Ok(()) => {
                self.metrics
                    .llm_jobs_enqueued
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                self.metrics
                    .critic_queue_depth
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            Err(mpsc::error::TrySendError::Full(run_id)) => {
                warn!(
                    run_id = %run_id,
                    "AgentMirror LLM critic queue full — retry on next idle sweep"
                );
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                warn!("AgentMirror LLM critic channel closed");
            }
        }
    }

    pub fn generate_daily_for_date(
        &self,
        date: NaiveDate,
    ) -> anyhow::Result<(DailyGenerateOutcome, Option<crate::models::DailyReport>)> {
        let cfg = self.config.read().clone();
        let llm = self.llm.lock().clone();
        let (outcome, report) = try_generate_daily_report(
            &self.store,
            date,
            llm.as_deref(),
            cfg.llm_daily,
            cfg.report_language(),
        )?;
        if outcome == DailyGenerateOutcome::Generated {
            if let Some(ref report) = report {
                persist_daily_report(&self.store, report)?;
                self.metrics.bump_activity();
            }
        }
        Ok((outcome, report))
    }

    pub fn reset(&self) -> anyhow::Result<crate::store::ResetStats> {
        self.store.reset_all()
    }

    pub fn process_turn_sync(&self, turn: TraceTurn) -> anyhow::Result<()> {
        let store = Arc::clone(&self.store);
        let scanner = self.safety.lock().clone();
        let cfg = self.config.read().clone();
        if let Some(run_id) = Pipeline::new(store, scanner, cfg).process_turn(turn)? {
            self.enqueue_llm_report(&run_id);
        }
        Ok(())
    }

    pub fn finalize_replayed_runs(&self) -> anyhow::Result<usize> {
        let cfg = self.config.read().clone();
        let run_ids = crate::report::finalize_runs_for_llm_reports(&self.store, cfg.llm_critic)?;
        for run_id in &run_ids {
            self.enqueue_llm_report(run_id);
        }
        Ok(run_ids.len())
    }
}

fn persist_daily_report(store: &InsightStore, report: &crate::models::DailyReport) -> anyhow::Result<()> {
    store.save_daily_report(report)?;
    let daily_dir = store.daily_reports_dir();
    std::fs::create_dir_all(&daily_dir)?;
    let md = daily_report_markdown(report);
    let path = daily_dir.join(format!("{}_all.md", report.date));
    let _ = std::fs::write(path, md);
    Ok(())
}

fn spawn_worker(
    mut rx: mpsc::Receiver<TraceTurn>,
    critic_tx: mpsc::Sender<String>,
    store: Arc<InsightStore>,
    spill: Arc<SpillQueue>,
    metrics: SharedMetrics,
    safety: SafetySlot,
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
                loop {
                    while let Ok(Some(turn)) = spill.pop_oldest() {
                        metrics
                            .spill_recovered
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        process_one_turn(
                            turn,
                            &critic_tx,
                            &store,
                            &metrics,
                            &safety,
                            &config,
                        );
                    }

                    let turn = match rx.recv().await {
                        Some(t) => t,
                        None => break,
                    };
                    metrics
                        .queue_depth
                        .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                    process_one_turn(
                        turn,
                        &critic_tx,
                        &store,
                        &metrics,
                        &safety,
                        &config,
                    );
                }
            });
        })
        .expect("spawn AgentMirror worker thread");
}

fn process_one_turn(
    turn: TraceTurn,
    critic_tx: &mpsc::Sender<String>,
    store: &Arc<InsightStore>,
    metrics: &SharedMetrics,
    safety: &SafetySlot,
    config: &Arc<parking_lot::RwLock<InsightConfig>>,
) {
    let audit_id = turn.audit_id.clone();
    let store = Arc::clone(store);
    let scanner = safety.lock().clone();
    let cfg = config.read().clone();
    match Pipeline::new(store, scanner, cfg).process_turn(turn) {
        Ok(Some(run_id)) => {
            metrics.processed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            metrics.bump_activity();
            match critic_tx.try_send(run_id) {
                Ok(()) => {
                    metrics
                        .llm_jobs_enqueued
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    metrics
                        .critic_queue_depth
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                Err(mpsc::error::TrySendError::Full(run_id)) => {
                    warn!(
                        run_id = %run_id,
                        "AgentMirror LLM critic queue full after turn processing"
                    );
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {}
            }
            debug!(audit_id = %audit_id, "AgentMirror processed turn");
        }
        Ok(None) => {
            metrics.processed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            metrics.bump_activity();
            debug!(audit_id = %audit_id, "AgentMirror processed turn");
        }
        Err(err) => error!(?err, audit_id = %audit_id, "AgentMirror process error"),
    }
}

fn spawn_llm_critic_worker(
    mut critic_rx: mpsc::Receiver<String>,
    store: Arc<InsightStore>,
    safety: SafetySlot,
    llm: LlmSlot,
    metrics: SharedMetrics,
    config: Arc<parking_lot::RwLock<InsightConfig>>,
) {
    thread::Builder::new()
        .name("agentmirror-llm-critic".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("AgentMirror LLM critic runtime");
            rt.block_on(async move {
                while let Some(run_id) = critic_rx.recv().await {
                    metrics
                        .critic_queue_depth
                        .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                    let scanner = safety.lock().clone();
                    let llm_client = llm.lock().clone();
                    let cfg = config.read().clone();
                    match generate_llm_reflection_for_run(
                        &store,
                        &run_id,
                        scanner.as_deref(),
                        llm_client.as_deref(),
                        cfg.llm_critic,
                        cfg.report_language(),
                    ) {
                        Ok(true) => {
                            metrics
                                .llm_jobs_completed
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            metrics.bump_activity();
                            tracing::info!(
                                run_id = %run_id,
                                "AgentMirror LLM reflection report generated"
                            );
                        }
                        Ok(false) => {
                            metrics
                                .llm_jobs_failed
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            warn!(
                                run_id = %run_id,
                                "AgentMirror LLM reflection report unavailable"
                            );
                        }
                        Err(err) => {
                            metrics
                                .llm_jobs_failed
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            error!(?err, run_id = %run_id, "AgentMirror LLM critic job failed");
                        }
                    }
                }
            });
        })
        .expect("spawn AgentMirror LLM critic thread");
}

fn spawn_daily_scheduler(
    store: Arc<InsightStore>,
    llm: LlmSlot,
    metrics: SharedMetrics,
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
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    let cfg = config.read().clone();
                    if !cfg.enabled {
                        continue;
                    }
                    let now = Local::now();
                    if now.hour() != u32::from(cfg.daily_report_hour) {
                        continue;
                    }
                    let today = now.date_naive().to_string();
                    if store
                        .get_meta(META_LAST_DAILY_REPORT_DATE)
                        .ok()
                        .flatten()
                        .as_deref()
                        == Some(today.as_str())
                    {
                        continue;
                    }
                    let yesterday = now.date_naive() - chrono::Duration::days(1);
                    let llm_client = llm.lock().clone();
                    match try_generate_daily_report(
                        &store,
                        yesterday,
                        llm_client.as_deref(),
                        cfg.llm_daily,
                        cfg.report_language(),
                    ) {
                        Ok((DailyGenerateOutcome::Generated, report)) => {
                            if let Some(report) = report {
                                if let Err(err) = persist_daily_report(&store, &report) {
                                    warn!(?err, "AgentMirror scheduled daily report persist failed");
                                } else {
                                    let _ = store.set_meta(META_LAST_DAILY_REPORT_DATE, &today);
                                    metrics.bump_activity();
                                    tracing::info!(
                                        date = %report.date,
                                        "AgentMirror scheduled daily report generated"
                                    );
                                }
                            } else {
                                warn!(
                                    date = %yesterday,
                                    "AgentMirror scheduled daily report missing payload"
                                );
                            }
                        }
                        Ok((DailyGenerateOutcome::Unchanged, _)) => {
                            let _ = store.set_meta(META_LAST_DAILY_REPORT_DATE, &today);
                            tracing::debug!(
                                date = %yesterday,
                                "AgentMirror scheduled daily report skipped — already up to date"
                            );
                        }
                        Ok((DailyGenerateOutcome::NoRuns, _)) => {
                            let _ = store.set_meta(META_LAST_DAILY_REPORT_DATE, &today);
                        }
                        Ok((DailyGenerateOutcome::Failed, _)) => {
                            warn!(
                                date = %yesterday,
                                "AgentMirror scheduled daily report LLM failed"
                            );
                        }
                        Err(err) => {
                            warn!(?err, "AgentMirror scheduled daily report failed");
                        }
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
    critic_tx: mpsc::Sender<String>,
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
                    let cfg = config.read().clone();
                    if !cfg.enabled || !cfg.llm_critic {
                        continue;
                    }
                    match sweep_idle_running_runs(&store, cfg.llm_critic) {
                        Ok(run_ids) => {
                            for run_id in run_ids {
                                if critic_tx.try_send(run_id.clone()).is_ok() {
                                    tracing::debug!(
                                        run_id = %run_id,
                                        "AgentMirror idle run queued for LLM reflection"
                                    );
                                }
                            }
                        }
                        Err(err) => warn!(?err, "AgentMirror idle run sweep failed"),
                    }
                }
            });
        })
        .expect("spawn AgentMirror idle sweeper thread");
}
