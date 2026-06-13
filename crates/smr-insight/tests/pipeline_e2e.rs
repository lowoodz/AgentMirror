use chrono::Utc;
use smr_insight::{InsightService, TraceTurn};

#[tokio::test]
async fn processes_openai_turn_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let graphs = dir.path().join("graphs");
    let svc = InsightService::open(dir.path(), graphs, Default::default()).unwrap();

    let request = br#"{
        "model": "gpt-4",
        "messages": [
            {"role": "system", "content": "You are a coding agent."},
            {"role": "user", "content": "Fix the login bug in auth.rs"}
        ],
        "tools": [{"type": "function", "function": {"name": "Read", "description": "read file"}}]
    }"#;

    let response = br#"{
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "I'll inspect the auth module first.",
                "tool_calls": [{
                    "id": "c1",
                    "type": "function",
                    "function": {"name": "Read", "arguments": "{\"path\":\"auth.rs\"}"}
                }]
            },
            "finish_reason": "tool_calls"
        }]
    }"#;

    svc.submit_turn(TraceTurn {
        audit_id: "audit-test-1".into(),
        session_id: "session-1".into(),
        agent_header: None,
        timestamp: Utc::now(),
        request_body: request.to_vec(),
        response_body: response.to_vec(),
    });

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let store = svc.store();
    let agents = store.list_agents(10).unwrap();
    assert!(!agents.is_empty(), "expected at least one agent");

    let runs = store.list_runs(None, 10).unwrap();
    assert!(!runs.is_empty(), "expected at least one run");
    assert!(runs[0].turn_count >= 1);

    let events = store.list_events(&runs[0].run_id).unwrap();
    assert!(events.iter().any(|e| e.summary.contains("login") || e.summary.contains("Fix")));
    assert!(events.iter().any(|e| e.summary.contains("Read")));

    let graph = store.load_graph_json(&runs[0].run_id).unwrap();
    assert!(graph.is_some());
}
