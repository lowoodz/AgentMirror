use chrono::Utc;
use smr_insight::{InsightService, TraceTurn};

#[tokio::test]
async fn merge_and_split_runs() {
    let dir = tempfile::tempdir().unwrap();
    let graphs = dir.path().join("graphs");
    let svc = InsightService::open(dir.path(), graphs, Default::default()).unwrap();
    let store = svc.store();

    let body = br#"{"messages":[{"role":"user","content":"task A"}],"tools":[]}"#;
    let resp = br#"{"choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]}"#;

    svc.submit_turn(TraceTurn {
        audit_id: "audit-a1".into(),
        session_id: "sess-merge".into(),
        agent_header: None,
        timestamp: Utc::now(),
        request_body: body.to_vec(),
        response_body: resp.to_vec(),
    });
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    svc.submit_turn(TraceTurn {
        audit_id: "audit-a2".into(),
        session_id: "sess-merge".into(),
        agent_header: None,
        timestamp: Utc::now(),
        request_body: br#"{"messages":[{"role":"user","content":"new task: task B"}]}"#.to_vec(),
        response_body: resp.to_vec(),
    });
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    let runs = store.list_runs(None, 10).unwrap();
    assert!(runs.len() >= 2, "expected at least two runs");
    let target = runs[1].run_id.clone();
    let source = runs[0].run_id.clone();
    store.merge_runs(&target, &[source]).unwrap();
    let merged = store.list_runs(None, 10).unwrap();
    assert!(merged.iter().any(|r| r.run_id == target));

    let events = store.list_events(&target).unwrap();
    assert!(events.len() >= 2);
    let split_at = events.first().unwrap().seq;
    let new_id = store.split_run(&target, split_at).unwrap();
    assert_ne!(new_id, target);
    assert!(store.get_run(&new_id).unwrap().is_some());
}
