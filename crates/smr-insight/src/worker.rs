use std::path::Path;
use std::sync::Arc;

use chrono::{Local, NaiveDate, Timelike};
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

use crate::models::{InsightConfig, TraceTurn};
use crate::pipeline::Pipeline;
use crate::report::generate_daily_report;
use crate::store::InsightStore;

const QUEUE_CAPACITY: usize = 256;

pub struct InsightService {
    config: Arc<parking_lot::RwLock<InsightConfig>>,
    tx: mpsc::Sender<TraceTurn>,
    store: Arc<InsightStore>,
}

impl InsightService {
    pub fn open(
        data_dir: &Path,
        graphs_dir: std::path::PathBuf,
        config: InsightConfig,
    ) -> anyhow::Result<Arc<Self>> {
        let store = Arc::new(InsightStore::open(data_dir, graphs_dir)?);
        let (tx, rx) = mpsc::channel(QUEUE_CAPACITY);
        let config_slot = Arc::new(parking_lot::RwLock::new(config));

        spawn_worker(rx, Arc::clone(&store));
        spawn_daily_scheduler(Arc::clone(&store), Arc::clone(&config_slot));

        Ok(Arc::new(Self {
            config: config_slot,
            tx,
            store,
        }))
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
        let agents = self.store.list_agents(500)?;
        let mut count = 0;
        for agent in agents {
            if let Some(report) = generate_daily_report(&self.store, &agent.agent_id, date)? {
                self.store.save_daily_report(&report)?;
                count += 1;
            }
        }
        Ok(count)
    }
}

fn spawn_worker(mut rx: mpsc::Receiver<TraceTurn>, store: Arc<InsightStore>) {
    tokio::spawn(async move {
        while let Some(turn) = rx.recv().await {
            let audit_id = turn.audit_id.clone();
            let store = Arc::clone(&store);
            let result =
                tokio::task::spawn_blocking(move || Pipeline::new(store).process_turn(turn)).await;
            match result {
                Ok(Ok(())) => debug!(audit_id = %audit_id, "AgentMirror processed turn"),
                Ok(Err(err)) => error!(?err, audit_id = %audit_id, "AgentMirror process error"),
                Err(err) => error!(?err, "AgentMirror worker join error"),
            }
        }
    });
}

fn spawn_daily_scheduler(store: Arc<InsightStore>, config: Arc<parking_lot::RwLock<InsightConfig>>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            let hour = config.read().daily_report_hour;
            if Local::now().hour() != u32::from(hour) {
                continue;
            }
            let yesterday = (Local::now() - chrono::Duration::days(1)).date_naive();
            let agents = store.list_agents(500).unwrap_or_default();
            for agent in agents {
                if let Ok(Some(report)) = generate_daily_report(&store, &agent.agent_id, yesterday)
                {
                    let _ = store.save_daily_report(&report);
                }
            }
        }
    });
}
