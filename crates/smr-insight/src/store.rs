use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::models::{
    AgentRecord, AgentRunStats, CognitiveEvent, DailyReport, EventKind, ReflectionReport,
    RunActionSequence, RunRecord, RunStatus,
};

pub struct InsightStore {
    conn: Mutex<Connection>,
    graphs_dir: PathBuf,
}

impl InsightStore {
    pub fn open(data_dir: &Path, graphs_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        std::fs::create_dir_all(&graphs_dir)?;
        let db_path = data_dir.join("smr.db");
        let conn = Connection::open(&db_path).context("open insight sqlite db")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS insight_agents (
                agent_id TEXT PRIMARY KEY,
                display_name TEXT NOT NULL,
                agent_type TEXT NOT NULL,
                system_hash TEXT NOT NULL,
                tools_json TEXT NOT NULL,
                first_seen TEXT NOT NULL,
                last_seen TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS insight_runs (
                run_id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                started_at TEXT NOT NULL,
                ended_at TEXT,
                status TEXT NOT NULL,
                goal TEXT NOT NULL,
                turn_count INTEGER NOT NULL DEFAULT 0,
                messages_seen INTEGER NOT NULL DEFAULT 0,
                graph_path TEXT,
                FOREIGN KEY (agent_id) REFERENCES insight_agents(agent_id)
            );
            CREATE TABLE IF NOT EXISTS insight_events (
                id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                kind TEXT NOT NULL,
                summary TEXT NOT NULL,
                audit_id TEXT NOT NULL,
                confidence REAL NOT NULL,
                timestamp TEXT NOT NULL,
                payload_json TEXT,
                FOREIGN KEY (run_id) REFERENCES insight_runs(run_id)
            );
            CREATE TABLE IF NOT EXISTS insight_reports (
                run_id TEXT PRIMARY KEY,
                generated_at TEXT NOT NULL,
                report_json TEXT NOT NULL,
                FOREIGN KEY (run_id) REFERENCES insight_runs(run_id)
            );
            CREATE TABLE IF NOT EXISTS insight_daily_reports (
                date TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                report_json TEXT NOT NULL,
                PRIMARY KEY (date, agent_id)
            );
            CREATE INDEX IF NOT EXISTS idx_insight_runs_agent ON insight_runs(agent_id, started_at DESC);
            CREATE INDEX IF NOT EXISTS idx_insight_runs_session ON insight_runs(session_id, started_at DESC);
            CREATE INDEX IF NOT EXISTS idx_insight_events_run ON insight_events(run_id, seq);
            CREATE TABLE IF NOT EXISTS insight_processed_audits (
                audit_id TEXT PRIMARY KEY,
                processed_at TEXT NOT NULL
            );",
        )?;
        let _ = conn.execute(
            "ALTER TABLE insight_runs ADD COLUMN messages_seen INTEGER NOT NULL DEFAULT 0",
            [],
        );
        Ok(Self {
            conn: Mutex::new(conn),
            graphs_dir,
        })
    }

    pub fn graphs_dir(&self) -> &Path {
        &self.graphs_dir
    }

    pub fn daily_reports_dir(&self) -> PathBuf {
        self.graphs_dir.parent().map(|p| p.join("daily")).unwrap_or_else(|| {
            PathBuf::from("data/insight/daily")
        })
    }

    pub fn is_audit_processed(&self, audit_id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT 1 FROM insight_processed_audits WHERE audit_id = ?1 LIMIT 1",
        )?;
        Ok(stmt.exists(params![audit_id])?)
    }

    pub fn mark_audit_processed(&self, audit_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO insight_processed_audits (audit_id, processed_at) VALUES (?1, ?2)",
            params![audit_id, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn upsert_agent(&self, agent: &AgentRecord) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO insight_agents (agent_id, display_name, agent_type, system_hash, tools_json, first_seen, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(agent_id) DO UPDATE SET
               display_name = excluded.display_name,
               agent_type = excluded.agent_type,
               last_seen = excluded.last_seen",
            params![
                agent.agent_id,
                agent.display_name,
                agent.agent_type,
                agent.system_hash,
                agent.tools_json,
                agent.first_seen.to_rfc3339(),
                agent.last_seen.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn get_agent(&self, agent_id: &str) -> Result<Option<AgentRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT agent_id, display_name, agent_type, system_hash, tools_json, first_seen, last_seen
             FROM insight_agents WHERE agent_id = ?1",
        )?;
        let mut rows = stmt.query(params![agent_id])?;
        if let Some(row) = rows.next()? {
            return Ok(Some(map_agent(row)?));
        }
        Ok(None)
    }

    pub fn list_agents(&self, limit: usize) -> Result<Vec<AgentRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT agent_id, display_name, agent_type, system_hash, tools_json, first_seen, last_seen
             FROM insight_agents ORDER BY last_seen DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], map_agent)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn insert_run(&self, run: &RunRecord) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO insight_runs (run_id, agent_id, session_id, started_at, ended_at, status, goal, turn_count, messages_seen, graph_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                run.run_id,
                run.agent_id,
                run.session_id,
                run.started_at.to_rfc3339(),
                run.ended_at.map(|t| t.to_rfc3339()),
                run.status.as_str(),
                run.goal,
                run.turn_count,
                run.messages_seen,
                run.graph_path,
            ],
        )?;
        Ok(())
    }

    pub fn update_run(&self, run: &RunRecord) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE insight_runs SET ended_at = ?2, status = ?3, goal = ?4, turn_count = ?5, messages_seen = ?6, graph_path = ?7
             WHERE run_id = ?1",
            params![
                run.run_id,
                run.ended_at.map(|t| t.to_rfc3339()),
                run.status.as_str(),
                run.goal,
                run.turn_count,
                run.messages_seen,
                run.graph_path,
            ],
        )?;
        Ok(())
    }

    pub fn get_run(&self, run_id: &str) -> Result<Option<RunRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT run_id, agent_id, session_id, started_at, ended_at, status, goal, turn_count, messages_seen, graph_path
             FROM insight_runs WHERE run_id = ?1",
        )?;
        let mut rows = stmt.query(params![run_id])?;
        if let Some(row) = rows.next()? {
            return Ok(Some(map_run(row)?));
        }
        Ok(None)
    }

    pub fn list_runs(&self, agent_id: Option<&str>, limit: usize) -> Result<Vec<RunRecord>> {
        let conn = self.conn.lock().unwrap();
        if let Some(agent_id) = agent_id {
            let mut stmt = conn.prepare(
                "SELECT run_id, agent_id, session_id, started_at, ended_at, status, goal, turn_count, messages_seen, graph_path
                 FROM insight_runs WHERE agent_id = ?1 ORDER BY started_at DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![agent_id, limit as i64], map_run)?;
            Ok(rows.filter_map(|r| r.ok()).collect())
        } else {
            let mut stmt = conn.prepare(
                "SELECT run_id, agent_id, session_id, started_at, ended_at, status, goal, turn_count, messages_seen, graph_path
                 FROM insight_runs ORDER BY started_at DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![limit as i64], map_run)?;
            Ok(rows.filter_map(|r| r.ok()).collect())
        }
    }

    pub fn find_active_run(&self, agent_id: &str, session_id: &str) -> Result<Option<RunRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT run_id, agent_id, session_id, started_at, ended_at, status, goal, turn_count, messages_seen, graph_path
             FROM insight_runs
             WHERE agent_id = ?1 AND session_id = ?2
             ORDER BY started_at DESC LIMIT 1",
        )?;
        let mut rows = stmt.query(params![agent_id, session_id])?;
        if let Some(row) = rows.next()? {
            return Ok(Some(map_run(row)?));
        }
        Ok(None)
    }

    /// Latest run for a proxy session (used when agent fingerprint drifted mid-conversation).
    pub fn find_active_run_for_session(&self, session_id: &str) -> Result<Option<RunRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT run_id, agent_id, session_id, started_at, ended_at, status, goal, turn_count, messages_seen, graph_path
             FROM insight_runs
             WHERE session_id = ?1
             ORDER BY started_at DESC LIMIT 1",
        )?;
        let mut rows = stmt.query(params![session_id])?;
        if let Some(row) = rows.next()? {
            return Ok(Some(map_run(row)?));
        }
        Ok(None)
    }

    pub fn insert_event(&self, event: &CognitiveEvent) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO insight_events (id, run_id, seq, kind, summary, audit_id, confidence, timestamp, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                event.id,
                event.run_id,
                event.seq,
                event.kind.as_str(),
                event.summary,
                event.audit_id,
                event.confidence,
                event.timestamp.to_rfc3339(),
                event.metadata.to_string(),
            ],
        )?;
        Ok(())
    }

    pub fn list_events(&self, run_id: &str) -> Result<Vec<CognitiveEvent>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, run_id, seq, kind, summary, audit_id, confidence, timestamp, payload_json
             FROM insight_events WHERE run_id = ?1 ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map(params![run_id], map_event)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn next_event_seq(&self, run_id: &str) -> Result<u32> {
        let conn = self.conn.lock().unwrap();
        let max: i64 = conn.query_row(
            "SELECT COALESCE(MAX(seq), -1) FROM insight_events WHERE run_id = ?1",
            params![run_id],
            |row| row.get(0),
        )?;
        Ok((max + 1) as u32)
    }

    pub fn save_report(&self, report: &ReflectionReport) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let json = serde_json::to_string(report)?;
        conn.execute(
            "INSERT OR REPLACE INTO insight_reports (run_id, generated_at, report_json)
             VALUES (?1, ?2, ?3)",
            params![report.run_id, report.generated_at.to_rfc3339(), json],
        )?;
        Ok(())
    }

    pub fn get_report(&self, run_id: &str) -> Result<Option<ReflectionReport>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT report_json FROM insight_reports WHERE run_id = ?1",
        )?;
        let mut rows = stmt.query(params![run_id])?;
        if let Some(row) = rows.next()? {
            let json: String = row.get(0)?;
            return Ok(Some(serde_json::from_str(&json)?));
        }
        Ok(None)
    }

    pub fn save_daily_report(&self, report: &DailyReport) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let json = serde_json::to_string(report)?;
        conn.execute(
            "INSERT OR REPLACE INTO insight_daily_reports (date, agent_id, report_json)
             VALUES (?1, ?2, ?3)",
            params![report.date, report.agent_id, json],
        )?;
        Ok(())
    }

    pub fn get_daily_report(&self, date: &str, agent_id: Option<&str>) -> Result<Vec<DailyReport>> {
        let conn = self.conn.lock().unwrap();
        if let Some(agent_id) = agent_id {
            let mut stmt = conn.prepare(
                "SELECT report_json FROM insight_daily_reports WHERE date = ?1 AND agent_id = ?2",
            )?;
            let mut rows = stmt.query(params![date, agent_id])?;
            if let Some(row) = rows.next()? {
                let json: String = row.get(0)?;
                return Ok(vec![serde_json::from_str(&json)?]);
            }
            return Ok(Vec::new());
        }
        let mut stmt = conn.prepare(
            "SELECT report_json FROM insight_daily_reports WHERE date = ?1 ORDER BY agent_id",
        )?;
        let rows = stmt.query_map(params![date], |row| {
            let json: String = row.get(0)?;
            Ok(json)
        })?;
        Ok(rows
            .filter_map(|r| r.ok())
            .filter_map(|json| serde_json::from_str::<DailyReport>(&json).ok())
            .collect())
    }

    pub fn runs_for_agent_on_date(&self, agent_id: &str, date: NaiveDate) -> Result<Vec<RunRecord>> {
        let start = date.and_hms_opt(0, 0, 0).unwrap().and_utc();
        let end = date.and_hms_opt(23, 59, 59).unwrap().and_utc();
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT run_id, agent_id, session_id, started_at, ended_at, status, goal, turn_count, messages_seen, graph_path
             FROM insight_runs
             WHERE agent_id = ?1 AND started_at >= ?2 AND started_at <= ?3
             ORDER BY started_at ASC",
        )?;
        let rows = stmt.query_map(
            params![agent_id, start.to_rfc3339(), end.to_rfc3339()],
            map_run,
        )?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn graph_path_for_run(&self, run_id: &str) -> PathBuf {
        self.graphs_dir.join(format!("{run_id}.json"))
    }

    pub fn save_graph_json(&self, run_id: &str, json: &str) -> Result<String> {
        let path = self.graph_path_for_run(run_id);
        std::fs::write(&path, json)?;
        Ok(path.display().to_string())
    }

    pub fn load_graph_json(&self, run_id: &str) -> Result<Option<String>> {
        let path = self.graph_path_for_run(run_id);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(std::fs::read_to_string(path)?))
    }

    pub fn update_run_goal(&self, run_id: &str, goal: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE insight_runs SET goal = ?2 WHERE run_id = ?1",
            params![run_id, goal],
        )?;
        Ok(())
    }

    pub fn merge_runs(&self, target_run_id: &str, source_run_ids: &[String]) -> Result<()> {
        if source_run_ids.is_empty() {
            return Ok(());
        }
        if !self.get_run(target_run_id)?.is_some() {
            anyhow::bail!("target run not found");
        }

        for source_id in source_run_ids {
            if source_id == target_run_id {
                continue;
            }
            if self.get_run(source_id)?.is_none() {
                continue;
            }
            let conn = self.conn.lock().unwrap();
            conn.execute(
                "UPDATE insight_events SET run_id = ?1 WHERE run_id = ?2",
                params![target_run_id, source_id],
            )?;
            drop(conn);
            let _ = std::fs::remove_file(self.graph_path_for_run(source_id));
            let conn = self.conn.lock().unwrap();
            conn.execute("DELETE FROM insight_reports WHERE run_id = ?1", params![source_id])?;
            conn.execute("DELETE FROM insight_runs WHERE run_id = ?1", params![source_id])?;
        }

        self.renumber_events(target_run_id)?;
        self.refresh_run_stats(target_run_id)?;
        Ok(())
    }

    pub fn split_run(&self, run_id: &str, after_seq: u32) -> Result<String> {
        let run = self
            .get_run(run_id)?
            .ok_or_else(|| anyhow::anyhow!("run not found"))?;
        let events = self.list_events(run_id)?;
        if events.iter().all(|e| e.seq <= after_seq) {
            anyhow::bail!("nothing to split after seq {after_seq}");
        }

        let new_run_id = crate::separator::new_run_id(&run.session_id, &run.agent_id);
        let split_goal = events
            .iter()
            .find(|e| e.seq > after_seq)
            .map(|e| e.summary.clone())
            .unwrap_or_else(|| run.goal.clone());

        let new_run = RunRecord {
            run_id: new_run_id.clone(),
            agent_id: run.agent_id.clone(),
            session_id: run.session_id.clone(),
            started_at: run.started_at,
            ended_at: run.ended_at,
            status: run.status,
            goal: split_goal,
            turn_count: 0,
            messages_seen: 0,
            graph_path: None,
        };
        self.insert_run(&new_run)?;

        {
            let conn = self.conn.lock().unwrap();
            conn.execute(
                "UPDATE insight_events SET run_id = ?1 WHERE run_id = ?2 AND seq > ?3",
                params![new_run_id, run_id, after_seq],
            )?;
        }

        self.renumber_events(run_id)?;
        self.renumber_events(&new_run_id)?;
        self.refresh_run_stats(run_id)?;
        self.refresh_run_stats(&new_run_id)?;
        Ok(new_run_id)
    }

    fn renumber_events(&self, run_id: &str) -> Result<()> {
        let mut events = self.list_events(run_id)?;
        events.sort_by_key(|e| e.seq);
        let conn = self.conn.lock().unwrap();
        for (idx, event) in events.iter().enumerate() {
            conn.execute(
                "UPDATE insight_events SET seq = ?2 WHERE id = ?1",
                params![event.id, idx as i64],
            )?;
        }
        Ok(())
    }

    fn refresh_run_stats(&self, run_id: &str) -> Result<()> {
        let mut run = self
            .get_run(run_id)?
            .ok_or_else(|| anyhow::anyhow!("run not found"))?;
        let events = self.list_events(run_id)?;
        run.turn_count = events
            .iter()
            .map(|e| e.audit_id.as_str())
            .collect::<std::collections::HashSet<_>>()
            .len() as u32;
        if run.turn_count == 0 {
            run.turn_count = 1;
        }
        let graph = crate::graph::build_graph(run_id, &events);
        let graph_json = serde_json::to_string_pretty(&graph)?;
        run.graph_path = Some(self.save_graph_json(run_id, &graph_json)?);
        self.update_run(&run)?;
        Ok(())
    }

    /// Remove insight data older than `retention_days` (0 = skip).
    pub fn purge_older_than(&self, retention_days: u32) -> Result<PurgeStats> {
        if retention_days == 0 {
            return Ok(PurgeStats::default());
        }
        let cutoff = Utc::now() - chrono::Duration::days(retention_days as i64);
        let cutoff_str = cutoff.to_rfc3339();
        let cutoff_date = cutoff.date_naive().to_string();

        let conn = self.conn.lock().unwrap();

        let mut run_ids: Vec<(String, Option<String>)> = Vec::new();
        {
            let mut stmt = conn.prepare(
                "SELECT run_id, graph_path FROM insight_runs WHERE started_at < ?1",
            )?;
            let rows = stmt.query_map(params![cutoff_str], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?;
            run_ids.extend(rows.filter_map(|r| r.ok()));
        }

        let mut stats = PurgeStats {
            runs: run_ids.len(),
            ..Default::default()
        };

        for (run_id, graph_path) in &run_ids {
            if let Some(path) = graph_path {
                let _ = std::fs::remove_file(path);
            } else {
                let _ = std::fs::remove_file(self.graph_path_for_run(run_id));
            }
            let events = conn.execute(
                "DELETE FROM insight_events WHERE run_id = ?1",
                params![run_id],
            )?;
            stats.events += events;
            let reports = conn.execute(
                "DELETE FROM insight_reports WHERE run_id = ?1",
                params![run_id],
            )?;
            stats.reports += reports;
        }

        conn.execute(
            "DELETE FROM insight_runs WHERE started_at < ?1",
            params![cutoff_str],
        )?;

        let daily = conn.execute(
            "DELETE FROM insight_daily_reports WHERE date < ?1",
            params![cutoff_date],
        )?;
        stats.daily_reports = daily;

        Ok(stats)
    }

    /// Wipe all AgentMirror tables and on-disk graph/daily files.
    /// Does not touch `audits`, `events`, or traffic snapshot files.
    pub fn reset_all(&self) -> Result<ResetStats> {
        let mut stats = ResetStats::default();

        if self.graphs_dir.exists() {
            for entry in std::fs::read_dir(&self.graphs_dir)? {
                let entry = entry?;
                if entry.path().extension().is_some_and(|e| e == "json") {
                    if entry.file_type()?.is_file() {
                        let _ = std::fs::remove_file(entry.path());
                        stats.graph_files += 1;
                    }
                }
            }
        }

        let daily_dir = self.daily_reports_dir();
        if daily_dir.exists() {
            for entry in std::fs::read_dir(&daily_dir)? {
                let entry = entry?;
                if entry.path().extension().is_some_and(|e| e == "md") {
                    if entry.file_type()?.is_file() {
                        let _ = std::fs::remove_file(entry.path());
                        stats.daily_files += 1;
                    }
                }
            }
        }

        let conn = self.conn.lock().unwrap();
        stats.events = conn.execute("DELETE FROM insight_events", [])?;
        stats.reports = conn.execute("DELETE FROM insight_reports", [])?;
        stats.daily_reports = conn.execute("DELETE FROM insight_daily_reports", [])?;
        stats.processed_audits =
            conn.execute("DELETE FROM insight_processed_audits", [])?;
        stats.runs = conn.execute("DELETE FROM insight_runs", [])?;
        stats.agents = conn.execute("DELETE FROM insight_agents", [])?;

        Ok(stats)
    }

    pub fn agent_run_stats(&self, agent_id: &str) -> Result<AgentRunStats> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT status, turn_count FROM insight_runs WHERE agent_id = ?1",
        )?;
        let rows = stmt.query_map(params![agent_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))
        })?;
        let mut stats = AgentRunStats {
            total_runs: 0,
            completed: 0,
            failed: 0,
            running: 0,
            stale: 0,
            total_turns: 0,
            avg_turns: 0.0,
        };
        for row in rows.flatten() {
            stats.total_runs += 1;
            stats.total_turns += row.1;
            match row.0.as_str() {
                "completed" => stats.completed += 1,
                "failed" => stats.failed += 1,
                "stale" => stats.stale += 1,
                _ => stats.running += 1,
            }
        }
        if stats.total_runs > 0 {
            stats.avg_turns = stats.total_turns as f32 / stats.total_runs as f32;
        }
        Ok(stats)
    }

    pub fn list_action_sequences(
        &self,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<RunActionSequence>> {
        let runs = self.list_runs(Some(agent_id), limit)?;
        let mut out = Vec::new();
        for run in runs {
            if !matches!(run.status, RunStatus::Completed | RunStatus::Failed) {
                continue;
            }
            let events = self.list_events(&run.run_id)?;
            let actions: Vec<String> = events
                .iter()
                .filter(|e| e.kind == EventKind::Action)
                .map(|e| e.summary.clone())
                .collect();
            if actions.is_empty() {
                continue;
            }
            out.push(RunActionSequence {
                run_id: run.run_id,
                status: run.status,
                actions,
            });
        }
        Ok(out)
    }

    pub fn audit_ids_for_run(&self, run_id: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT audit_id FROM insight_events WHERE run_id = ?1 AND audit_id != ''",
        )?;
        let rows = stmt.query_map(params![run_id], |row| row.get::<_, String>(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn audit_ids_for_runs(&self, run_ids: &[String]) -> Result<Vec<String>> {
        if run_ids.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock().unwrap();
        let placeholders: String = run_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT DISTINCT audit_id FROM insight_events WHERE run_id IN ({placeholders}) AND audit_id != ''"
        );
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> = run_ids
            .iter()
            .map(|id| id as &dyn rusqlite::ToSql)
            .collect();
        let rows = stmt.query_map(params.as_slice(), |row| row.get::<_, String>(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PurgeStats {
    pub runs: usize,
    pub events: usize,
    pub reports: usize,
    pub daily_reports: usize,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ResetStats {
    pub agents: usize,
    pub runs: usize,
    pub events: usize,
    pub reports: usize,
    pub daily_reports: usize,
    pub processed_audits: usize,
    pub graph_files: usize,
    pub daily_files: usize,
}

fn parse_ts(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn map_agent(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentRecord> {
    Ok(AgentRecord {
        agent_id: row.get(0)?,
        display_name: row.get(1)?,
        agent_type: row.get(2)?,
        system_hash: row.get(3)?,
        tools_json: row.get(4)?,
        first_seen: parse_ts(&row.get::<_, String>(5)?),
        last_seen: parse_ts(&row.get::<_, String>(6)?),
    })
}

fn map_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunRecord> {
    let status: String = row.get(5)?;
    let status = match status.as_str() {
        "completed" => RunStatus::Completed,
        "failed" => RunStatus::Failed,
        "stale" => RunStatus::Stale,
        _ => RunStatus::Running,
    };
    let ended: Option<String> = row.get(4)?;
    Ok(RunRecord {
        run_id: row.get(0)?,
        agent_id: row.get(1)?,
        session_id: row.get(2)?,
        started_at: parse_ts(&row.get::<_, String>(3)?),
        ended_at: ended.map(|s| parse_ts(&s)),
        status,
        goal: row.get(6)?,
        turn_count: row.get::<_, i64>(7)? as u32,
        messages_seen: row.get::<_, i64>(8)? as u32,
        graph_path: row.get(9)?,
    })
}

fn map_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<CognitiveEvent> {
    let kind: String = row.get(3)?;
    let kind = match kind.as_str() {
        "goal" => EventKind::Goal,
        "sub_goal" => EventKind::SubGoal,
        "decision" => EventKind::Decision,
        "action" => EventKind::Action,
        "observation" => EventKind::Observation,
        "reflection" => EventKind::Reflection,
        "result" => EventKind::Result,
        "state_transition" => EventKind::StateTransition,
        _ => EventKind::Action,
    };
    let payload: Option<String> = row.get(8)?;
    Ok(CognitiveEvent {
        id: row.get(0)?,
        run_id: row.get(1)?,
        seq: row.get::<_, i64>(2)? as u32,
        kind,
        summary: row.get(4)?,
        audit_id: row.get(5)?,
        confidence: row.get(6)?,
        timestamp: parse_ts(&row.get::<_, String>(7)?),
        metadata: payload
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::Value::Null),
    })
}
