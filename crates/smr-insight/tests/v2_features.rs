use chrono::{NaiveDate, TimeZone, Utc};
use smr_insight::models::{AgentRecord, CognitiveEvent, EventKind, RunRecord, RunStatus};
use smr_insight::pattern::mine_patterns;
use smr_insight::profile::build_profile;
use smr_insight::store::InsightStore;
use uuid::Uuid;

fn open_store() -> (tempfile::TempDir, InsightStore) {
    let dir = tempfile::tempdir().unwrap();
    let graphs = dir.path().join("graphs");
    let store = InsightStore::open(dir.path(), graphs).unwrap();
    (dir, store)
}

fn seed_agent(store: &InsightStore, agent_id: &str) {
    store
        .upsert_agent(&AgentRecord {
            agent_id: agent_id.to_string(),
            display_name: "Test Agent".to_string(),
            agent_type: "coding".to_string(),
            system_hash: "abc123".to_string(),
            tools_json: r#"["Read","Bash","Edit"]"#.to_string(),
            first_seen: Utc::now(),
            last_seen: Utc::now(),
        })
        .unwrap();
}

fn seed_run_with_actions(
    store: &InsightStore,
    agent_id: &str,
    run_id: &str,
    status: RunStatus,
    actions: &[&str],
) {
    store
        .insert_run(&RunRecord {
            run_id: run_id.to_string(),
            agent_id: agent_id.to_string(),
            session_id: "sess-1".to_string(),
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
            status,
            goal: "Fix login bug".to_string(),
            turn_count: actions.len() as u32,
            messages_seen: 0,
            graph_path: None,
        })
        .unwrap();
    for (i, summary) in actions.iter().enumerate() {
        store
            .insert_event(&CognitiveEvent {
                id: Uuid::new_v4().to_string(),
                run_id: run_id.to_string(),
                seq: i as u32,
                kind: EventKind::Action,
                timestamp: Utc::now(),
                summary: (*summary).to_string(),
                audit_id: format!("audit-{run_id}-{i}"),
                confidence: 0.9,
                metadata: serde_json::Value::Null,
            })
            .unwrap();
    }
}

#[test]
fn agent_run_stats_and_profile() {
    let (_dir, store) = open_store();
    let agent_id = "agent-v2";
    seed_agent(&store, agent_id);
    seed_run_with_actions(
        &store,
        agent_id,
        "run-ok",
        RunStatus::Completed,
        &["read file", "apply patch"],
    );
    seed_run_with_actions(
        &store,
        agent_id,
        "run-fail",
        RunStatus::Failed,
        &["read file", "run tests"],
    );

    let stats = store.agent_run_stats(agent_id).unwrap();
    assert_eq!(stats.total_runs, 2);
    assert_eq!(stats.completed, 1);
    assert_eq!(stats.failed, 1);

    let agent = store.get_agent(agent_id).unwrap().unwrap();
    let profile = build_profile(&agent, stats);
    assert!(profile.capabilities.contains(&"code_editing".to_string()));
    assert!(profile.tools.contains(&"Read".to_string()));
}

#[test]
fn pattern_mining_from_store_sequences() {
    let (_dir, store) = open_store();
    let agent_id = "agent-pat";
    seed_agent(&store, agent_id);
    seed_run_with_actions(
        &store,
        agent_id,
        "run-a",
        RunStatus::Completed,
        &["read file", "apply patch", "verify"],
    );
    seed_run_with_actions(
        &store,
        agent_id,
        "run-b",
        RunStatus::Completed,
        &["read file", "apply patch", "commit"],
    );
    seed_run_with_actions(
        &store,
        agent_id,
        "run-c",
        RunStatus::Failed,
        &["read file", "apply patch", "verify"],
    );

    let sequences = store.list_action_sequences(agent_id, 50).unwrap();
    assert_eq!(sequences.len(), 3);
    let patterns = mine_patterns(&sequences);
    assert!(!patterns.is_empty());
    assert!(patterns.iter().any(|p| p.steps.len() >= 2));
}

#[test]
fn audit_ids_for_runs() {
    let (_dir, store) = open_store();
    let agent_id = "agent-audit";
    seed_agent(&store, agent_id);
    seed_run_with_actions(
        &store,
        agent_id,
        "run-x",
        RunStatus::Completed,
        &["action one"],
    );
    let ids = store.audit_ids_for_run("run-x").unwrap();
    assert_eq!(ids.len(), 1);
    assert!(ids[0].starts_with("audit-run-x"));
    let batch = store.audit_ids_for_runs(&["run-x".to_string()]).unwrap();
    assert_eq!(batch.len(), 1);
    let map = store
        .audit_ids_map_for_runs(&["run-x".to_string()])
        .unwrap();
    assert_eq!(map.get("run-x").map(|v| v.len()), Some(1));
}

#[test]
fn runs_on_date_includes_cross_midnight_activity() {
    let (_dir, store) = open_store();
    let agent_id = "agent-midnight";
    seed_agent(&store, agent_id);
    let day1 = NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();
    let day2 = NaiveDate::from_ymd_opt(2026, 6, 11).unwrap();
    let started = Utc.with_ymd_and_hms(2026, 6, 10, 23, 50, 0).unwrap();
    let ended = Utc.with_ymd_and_hms(2026, 6, 11, 0, 30, 0).unwrap();
    store
        .insert_run(&RunRecord {
            run_id: "run-midnight".to_string(),
            agent_id: agent_id.to_string(),
            session_id: "sess-m".to_string(),
            started_at: started,
            ended_at: Some(ended),
            status: RunStatus::Completed,
            goal: "overnight task".to_string(),
            turn_count: 2,
            messages_seen: 0,
            graph_path: None,
        })
        .unwrap();

    let day1_runs = store.runs_on_date(day1).unwrap();
    assert!(day1_runs.iter().any(|r| r.run_id == "run-midnight"));

    let day2_runs = store.runs_on_date(day2).unwrap();
    assert!(
        day2_runs.iter().any(|r| r.run_id == "run-midnight"),
        "cross-midnight run should appear on the day it ended"
    );
}

#[test]
fn agents_on_date_lists_only_agents_with_runs_that_day() {
    let (_dir, store) = open_store();
    seed_agent(&store, "agent-a");
    seed_agent(&store, "agent-b");
    let day = NaiveDate::from_ymd_opt(2026, 6, 12).unwrap();
    let started = Utc.with_ymd_and_hms(2026, 6, 12, 10, 0, 0).unwrap();
    store
        .insert_run(&RunRecord {
            run_id: "run-a".to_string(),
            agent_id: "agent-a".to_string(),
            session_id: "sess-a".to_string(),
            started_at: started,
            ended_at: Some(started + chrono::Duration::minutes(5)),
            status: RunStatus::Completed,
            goal: "task a".to_string(),
            turn_count: 1,
            messages_seen: 0,
            graph_path: None,
        })
        .unwrap();

    let agents = store.agents_on_date(day).unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].agent_id, "agent-a");
}
