use chrono::Utc;
use smr_insight::{InsightService, TraceTurn};

/// When session keys drift (e.g. growing system prompt before tools-based anchor),
/// duplicate Task Runs with the same goal should merge via find_duplicate_run.
#[tokio::test]
async fn deduplicates_runs_with_same_goal_across_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let graphs = dir.path().join("graphs");
    let svc = InsightService::open(dir.path(), graphs, Default::default()).unwrap();

    let turn1_req = r#"{
        "messages": [
            {"role": "user", "content": "帮我调研一下珠海金智维，看看是否值得投资"}
        ],
        "tools": [{"type": "function", "function": {"name": "exec"}}]
    }"#
    .as_bytes();
    let turn1_resp = r#"{
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "我先搜索珠海金智维的基本信息。",
                "tool_calls": [{
                    "id": "c1",
                    "type": "function",
                    "function": {"name": "exec", "arguments": "{\"command\":\"curl example\"}"}
                }]
            },
            "finish_reason": "tool_calls"
        }]
    }"#
    .as_bytes();

    svc.submit_turn(TraceTurn {
        audit_id: "dedup-a1".into(),
        session_id: "session-old-key".into(),
        agent_header: None,
        timestamp: Utc::now(),
        request_body: turn1_req.to_vec(),
        response_body: turn1_resp.to_vec(),
    });
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    let turn2_req = r#"{
        "messages": [
            {"role": "user", "content": "帮我调研一下珠海金智维，看看是否值得投资"},
            {"role": "assistant", "content": "我先搜索珠海金智维的基本信息。"},
            {"role": "tool", "content": "珠海金智维信息科技有限公司，主营 RPA。"}
        ],
        "tools": [{"type": "function", "function": {"name": "exec"}}]
    }"#
    .as_bytes();
    let turn2_resp = r#"{
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "接下来继续查询融资情况。"
            },
            "finish_reason": "stop"
        }]
    }"#
    .as_bytes();

    svc.submit_turn(TraceTurn {
        audit_id: "dedup-a2".into(),
        session_id: "session-new-key".into(),
        agent_header: None,
        timestamp: Utc::now(),
        request_body: turn2_req.to_vec(),
        response_body: turn2_resp.to_vec(),
    });
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    let store = svc.store();
    let runs = store.list_runs(None, 10).unwrap();
    assert_eq!(
        runs.len(),
        1,
        "same goal within idle window should stay one run: {:?}",
        runs.iter().map(|r| (&r.run_id, &r.goal)).collect::<Vec<_>>()
    );
    assert_eq!(runs[0].turn_count, 2);
}
