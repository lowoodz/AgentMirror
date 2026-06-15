use chrono::Utc;
use smr_insight::models::EventKind;
use smr_insight::{InsightService, TraceTurn};

#[tokio::test]
async fn multi_turn_session_deduplicates_history_and_builds_graph() {
    let dir = tempfile::tempdir().unwrap();
    let graphs = dir.path().join("graphs");
    let svc = InsightService::open(dir.path(), graphs, Default::default()).unwrap();

    let turn1_req = r#"{
        "messages": [
            {"role": "user", "content": "Compare three Rust HTTP client libraries and recommend one"}
        ],
        "tools": [{"type": "function", "function": {"name": "exec", "description": "run shell"}}]
    }"#
    .as_bytes();
    let turn1_resp = r#"{
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "我先搜索这三个库的最新文档和社区评价。",
                "tool_calls": [{
                    "id": "c1",
                    "type": "function",
                    "function": {"name": "exec", "arguments": "{\"command\":\"curl https://example.com/search?q=rust+http+clients\"}"}
                }]
            },
            "finish_reason": "tool_calls"
        }]
    }"#
    .as_bytes();

    svc.submit_turn(TraceTurn {
        audit_id: "audit-r1".into(),
        session_id: "session-explore".into(),
        agent_header: None,
        timestamp: Utc::now(),
        request_body: turn1_req.to_vec(),
        response_body: turn1_resp.to_vec(),
    });
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    let turn2_req = r#"{
        "messages": [
            {"role": "user", "content": "Compare three Rust HTTP client libraries and recommend one"},
            {"role": "assistant", "content": "我先搜索这三个库的最新文档和社区评价。"},
            {"role": "tool", "content": "reqwest: async-first, widely used. ureq: blocking, minimal deps. hyper: low-level building block."}
        ],
        "tools": [{"type": "function", "function": {"name": "exec"}}]
    }"#
    .as_bytes();
    let turn2_resp = r#"{
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "接下来我会继续查询各库的维护活跃度。",
                "tool_calls": [{
                    "id": "c2",
                    "type": "function",
                    "function": {"name": "exec", "arguments": "{\"command\":\"curl https://example.com/search?q=crates.io+activity\"}"}
                }]
            },
            "finish_reason": "tool_calls"
        }]
    }"#
    .as_bytes();

    svc.submit_turn(TraceTurn {
        audit_id: "audit-r2".into(),
        session_id: "session-explore".into(),
        agent_header: None,
        timestamp: Utc::now(),
        request_body: turn2_req.to_vec(),
        response_body: turn2_resp.to_vec(),
    });
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    let turn3_req = turn2_req;
    let turn3_resp = r#"{
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "综合以上信息，reqwest 生态最成熟，ureq 适合简单同步场景，hyper 适合自定义栈。结论：默认推荐 reqwest，同步 CLI 可选 ureq。"
            },
            "finish_reason": "stop"
        }]
    }"#
    .as_bytes();

    svc.submit_turn(TraceTurn {
        audit_id: "audit-r3".into(),
        session_id: "session-explore".into(),
        agent_header: None,
        timestamp: Utc::now(),
        request_body: turn3_req.to_vec(),
        response_body: turn3_resp.to_vec(),
    });
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    let store = svc.store();
    let runs = store.list_runs(None, 10).unwrap();
    assert_eq!(runs.len(), 1, "multi-turn session should stay in one run");
    assert_eq!(runs[0].turn_count, 3);

    let events = store.list_events(&runs[0].run_id).unwrap();
    let goal_count = events
        .iter()
        .filter(|e| e.kind == EventKind::Goal)
        .count();
    assert_eq!(goal_count, 1, "should not duplicate goal across turns");

    assert!(
        events.iter().any(|e| e.summary.contains("WebSearch")),
        "exec with http args should normalize to WebSearch: {:?}",
        events.iter().map(|e| &e.summary).collect::<Vec<_>>()
    );
    assert!(
        events.iter().any(|e| e.kind == EventKind::Result),
        "explicit conclusion should be captured as Result"
    );

    let agents = store.list_agents(10).unwrap();
    assert!(
        agents.iter().any(|a| a.agent_type == "explore"),
        "OpenClaw exec-only agent should classify as explore from tools/platform"
    );

    let graph = store.load_graph_json(&runs[0].run_id).unwrap().unwrap();
    assert!(
        !graph.contains("\"to\":\"n0\""),
        "goal node should be root without incoming edges"
    );
}
