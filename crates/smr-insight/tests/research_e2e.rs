use chrono::Utc;
use smr_insight::models::EventKind;
use smr_insight::{InsightService, TraceTurn};

#[tokio::test]
async fn research_session_deduplicates_history_and_builds_graph() {
    let dir = tempfile::tempdir().unwrap();
    let graphs = dir.path().join("graphs");
    let svc = InsightService::open(dir.path(), graphs, Default::default()).unwrap();

    let turn1_req = r#"{
        "messages": [
            {"role": "user", "content": "帮我调研一下珠海金智维，看看是否值得投资"}
        ],
        "tools": [{"type": "function", "function": {"name": "exec", "description": "run shell"}}]
    }"#
    .as_bytes();
    let turn1_resp = r#"{
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "我先搜索珠海金智维的基本信息和主营业务。",
                "tool_calls": [{
                    "id": "c1",
                    "type": "function",
                    "function": {"name": "exec", "arguments": "{\"command\":\"curl https://example.com/search?q=zhuhai\"}"}
                }]
            },
            "finish_reason": "tool_calls"
        }]
    }"#
    .as_bytes();

    svc.submit_turn(TraceTurn {
        audit_id: "audit-r1".into(),
        session_id: "session-research".into(),
        agent_header: None,
        timestamp: Utc::now(),
        request_body: turn1_req.to_vec(),
        response_body: turn1_resp.to_vec(),
    });
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    let turn2_req = r#"{
        "messages": [
            {"role": "user", "content": "帮我调研一下珠海金智维，看看是否值得投资"},
            {"role": "assistant", "content": "我先搜索珠海金智维的基本信息和主营业务。"},
            {"role": "tool", "content": "珠海金智维信息科技有限公司，主营 RPA 与 AI 解决方案，成立于 2015 年。"}
        ],
        "tools": [{"type": "function", "function": {"name": "exec"}}]
    }"#
    .as_bytes();
    let turn2_resp = r#"{
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "接下来我会继续查询融资与竞争格局。",
                "tool_calls": [{
                    "id": "c2",
                    "type": "function",
                    "function": {"name": "exec", "arguments": "{\"command\":\"curl https://example.com/search?q=funding\"}"}
                }]
            },
            "finish_reason": "tool_calls"
        }]
    }"#
    .as_bytes();

    svc.submit_turn(TraceTurn {
        audit_id: "audit-r2".into(),
        session_id: "session-research".into(),
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
                "content": "综合以上调研，珠海金智维在 RPA 领域具备一定优势，但估值偏高、竞争加剧。结论：谨慎关注，暂不建议重仓投资，可等待下一轮融资后再评估。"
            },
            "finish_reason": "stop"
        }]
    }"#
    .as_bytes();

    svc.submit_turn(TraceTurn {
        audit_id: "audit-r3".into(),
        session_id: "session-research".into(),
        agent_header: None,
        timestamp: Utc::now(),
        request_body: turn3_req.to_vec(),
        response_body: turn3_resp.to_vec(),
    });
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    let store = svc.store();
    let runs = store.list_runs(None, 10).unwrap();
    assert_eq!(runs.len(), 1, "research session should stay in one run");
    assert_eq!(runs[0].turn_count, 3);

    let events = store.list_events(&runs[0].run_id).unwrap();
    let goal_count = events
        .iter()
        .filter(|e| e.kind == EventKind::Goal)
        .count();
    assert_eq!(goal_count, 1, "should not duplicate goal across turns");

    assert!(
        events.iter().any(|e| e.summary.contains("WebSearch")),
        "exec should normalize to WebSearch: {:?}",
        events.iter().map(|e| &e.summary).collect::<Vec<_>>()
    );
    assert!(
        events.iter().any(|e| e.kind == EventKind::Result),
        "investment conclusion should be captured as Result"
    );

    let agents = store.list_agents(10).unwrap();
    assert!(
        agents.iter().any(|a| a.agent_type == "research"),
        "OpenClaw exec-only agent with research goal should classify as research"
    );

    let graph = store.load_graph_json(&runs[0].run_id).unwrap().unwrap();
    assert!(
        !graph.contains("\"to\":\"n0\""),
        "goal node should be root without incoming edges"
    );
}
