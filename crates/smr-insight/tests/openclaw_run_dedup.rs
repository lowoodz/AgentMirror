use chrono::Utc;
use smr_insight::{InsightService, TraceTurn};

fn bootstrap_user(time_line: &str) -> String {
    format!(
        "A new session was started via /new or /reset. Execute your Session Startup sequence now. {time_line}"
    )
}

fn openclaw_body(user: &str, history: &str) -> String {
    format!(
        r#"{{
        "model": "routed",
        "messages": [
            {{"role": "system", "content": "You are OpenClaw"}},
            {history}
            {{"role": "user", "content": {user_json}}}
        ],
        "tools": [{{"type": "function", "function": {{"name": "exec"}}}}]
    }}"#,
        user_json = serde_json::to_string(user).unwrap(),
        history = history,
    )
}

/// OpenClaw bootstrap user text embeds a changing clock; must not split one agent into many runs.
#[tokio::test]
async fn openclaw_bootstrap_timestamp_drift_stays_in_one_run() {
    let dir = tempfile::tempdir().unwrap();
    let graphs = dir.path().join("graphs");
    let svc = InsightService::open(dir.path(), graphs, Default::default()).unwrap();

    let research = "帮我调研一下珠海金智维，看看是否值得投资";
    let sid = "session-openclaw-research";

    let turn1 = openclaw_body(&bootstrap_user("Current time: 4:58 PM"), "");
    let turn2 = openclaw_body(
        &bootstrap_user("Current time: 5:01 PM"),
        &format!(
            r#"{{"role": "user", "content": {}}}, {{"role": "assistant", "content": "hi"}}, "#,
            serde_json::to_string(&bootstrap_user("Current time: 4:58 PM")).unwrap()
        ),
    );
    let turn3 = openclaw_body(
        research,
        &format!(
            r#"{{"role": "user", "content": {}}}, {{"role": "assistant", "content": "hi"}}, {{"role": "user", "content": {}}}, {{"role": "assistant", "content": "searching"}}, "#,
            serde_json::to_string(&bootstrap_user("Current time: 4:58 PM")).unwrap(),
            serde_json::to_string(research).unwrap(),
        ),
    );

    for (audit, body) in [
        ("oc-1", turn1.as_bytes()),
        ("oc-2", turn2.as_bytes()),
        ("oc-3", turn3.as_bytes()),
    ] {
        svc.submit_turn(TraceTurn {
            audit_id: audit.into(),
            session_id: sid.into(),
            agent_header: None,
            timestamp: Utc::now(),
            request_body: body.to_vec(),
            response_body: br#"{"choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]}"#.to_vec(),
        });
        tokio::time::sleep(std::time::Duration::from_millis(350)).await;
    }

    let store = svc.store();
    let agent_id = store.list_agents(1).unwrap()[0].agent_id.clone();
    let runs = store.list_runs(Some(&agent_id), 20).unwrap();
    assert_eq!(
        runs.len(),
        1,
        "same agent + session must stay one run: {:?}",
        runs.iter().map(|r| (&r.goal, r.turn_count)).collect::<Vec<_>>()
    );
    assert_eq!(runs[0].turn_count, 3);
    assert!(
        runs[0].goal.contains("金智维"),
        "goal should reflect research task: {}",
        runs[0].goal
    );
}
