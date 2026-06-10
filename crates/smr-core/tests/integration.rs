use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use axum::http::Method;
use axum::routing::post;
use axum::{Json, Router};
use bytes::Bytes;
use smr_core::config::*;
use smr_core::events::EventLog;
use smr_core::storage::AuditStore;
use smr_core::proxy::ProxyService;
use smr_core::request::ProxyRequest;
use smr_core::state::SharedApp;
use smr_protocol::{extract_texts, parse_json_body};
use tempfile::NamedTempFile;
use std::io::Write;

fn workspace_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
}

fn test_config(upstream_base: &str) -> AppConfig {
    let mut groups = HashMap::new();
    groups.insert(
        "high".to_string(),
        vec![ModelEndpoint {
            id: "mock".into(),
            base_url: upstream_base.into(),
            model: "mock-model".into(),
            api_key: Some("test-key".into()),
            api_key_env: None,
            timeout_secs: 10,
            protocol: None,
        }],
    );

    AppConfig {
        server: ServerConfig {
            listen: "127.0.0.1:0".into(),
            default_fallback_group: "high".into(),
            ..Default::default()
        },
        pipeline: PipelineConfig {
            dlp_enabled: true,
            operation_security_mode: OperationSecurityMode::Enforce,
            ..Default::default()
        },
        logging: LoggingConfig::default(),
        fallback_groups: groups,
        content_rules: vec![ContentRule {
            id: "secret".into(),
            enabled: true,
            match_mode: MatchMode::Full,
            value: "TOP-SECRET-KEY".into(),
            category: ContentCategory::Secret,
            min_fragment_len: None,
            min_fragment_ratio: None,
        }],
        file_rules: vec![],
        operation_rules: vec![OperationRule {
            id: "block-rm".into(),
            enabled: true,
            operation: OperationType::CommandExec,
            object: OperationObject {
                pattern: "rm -rf".into(),
                is_regex: false,
            },
        }],
        path_protection_rules: vec![],
    }
}

fn make_app(config: AppConfig) -> (Arc<SharedApp>, ProxyService) {
    let mut tmp = NamedTempFile::new().unwrap();
    write!(tmp, "{}", serde_yaml::to_string(&config).unwrap()).unwrap();
    let storage = Arc::new(
        AuditStore::open(&std::env::temp_dir().join("smr-test-db")).unwrap(),
    );
    let app = SharedApp::new(
        tmp.path().to_path_buf(),
        config,
        EventLog::new(100),
        storage,
    )
    .unwrap();
    let proxy = ProxyService::new(app.clone());
    (app, proxy)
}

async fn spawn_mock_upstream(dangerous: bool) -> String {
    async fn handler(dangerous: bool) -> Json<serde_json::Value> {
        let args = if dangerous {
            r#"{"command":"rm -rf /"}"#
        } else {
            r#"{"command":"echo hello"}"#
        };
        Json(serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "run_terminal_cmd",
                            "arguments": args
                        }
                    }]
                }
            }]
        }))
    }

    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || handler(dangerous)),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.ok(); });
    format!("http://{addr}")
}

async fn spawn_malformed_upstream() -> String {
    async fn handler() -> axum::response::Response {
        axum::response::Response::builder()
            .status(axum::http::StatusCode::OK)
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from("{not-valid-json"))
            .unwrap()
    }

    let app = Router::new().route(
        "/v1/chat/completions",
        post(handler),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.ok(); });
    format!("http://{addr}")
}

#[tokio::test]
async fn malformed_json_triggers_fallback() {
    let bad = spawn_malformed_upstream().await;
    let good = spawn_mock_upstream(false).await;

    let mut groups = HashMap::new();
    groups.insert(
        "high".to_string(),
        vec![
            ModelEndpoint {
                id: "bad".into(),
                base_url: bad,
                model: "bad-model".into(),
                api_key: Some("k".into()),
                api_key_env: None,
                timeout_secs: 5,
                protocol: None,
            },
            ModelEndpoint {
                id: "good".into(),
                base_url: good,
                model: "mock-model".into(),
                api_key: Some("k".into()),
                api_key_env: None,
                timeout_secs: 10,
                protocol: None,
            },
        ],
    );

    let mut config = test_config("http://127.0.0.1:9");
    config.fallback_groups = groups;

    let (_app, proxy) = make_app(config);

    let body = serde_json::json!({
        "model": "mock-model",
        "messages": [{"role": "user", "content": "hello"}]
    });
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(axum::http::header::CONTENT_TYPE, "application/json".parse().unwrap());

    let (status, _, resp_body) = proxy
        .handle_api_request(ProxyRequest {
            session_id: "sess-malformed",
            fallback_group: Some("high"),
            method: Method::POST,
            path: "/v1/chat/completions",
            query: None,
            headers,
            body: Bytes::from(serde_json::to_vec(&body).unwrap()),
        })
        .await
        .unwrap();

    assert!(status.is_success());
    assert!(serde_json::from_slice::<serde_json::Value>(&extract_body_bytes(resp_body)).is_ok());
}

async fn spawn_not_found_upstream() -> String {
    use axum::http::StatusCode;

    async fn handler() -> StatusCode {
        StatusCode::NOT_FOUND
    }

    let app = Router::new().route("/v1/chat/completions", post(handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.ok(); });
    format!("http://{addr}")
}

#[tokio::test]
async fn upstream_404_triggers_fallback_to_next_model() {
    let bad = spawn_not_found_upstream().await;
    let good = spawn_mock_upstream(false).await;

    let mut groups = HashMap::new();
    groups.insert(
        "high".to_string(),
        vec![
            ModelEndpoint {
                id: "bad".into(),
                base_url: bad,
                model: "missing-model".into(),
                api_key: Some("k".into()),
                api_key_env: None,
                timeout_secs: 5,
                protocol: None,
            },
            ModelEndpoint {
                id: "good".into(),
                base_url: good,
                model: "mock-model".into(),
                api_key: Some("k".into()),
                api_key_env: None,
                timeout_secs: 10,
                protocol: None,
            },
        ],
    );

    let mut config = test_config("http://127.0.0.1:9");
    config.fallback_groups = groups;

    let (_app, proxy) = make_app(config);

    let body = serde_json::json!({
        "model": "saferoute-high",
        "messages": [{"role": "user", "content": "hello"}]
    });
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(axum::http::header::CONTENT_TYPE, "application/json".parse().unwrap());

    let (status, _, resp_body) = proxy
        .handle_api_request(ProxyRequest {
            session_id: "sess-404-fallback",
            fallback_group: Some("high"),
            method: Method::POST,
            path: "/v1/chat/completions",
            query: None,
            headers,
            body: Bytes::from(serde_json::to_vec(&body).unwrap()),
        })
        .await
        .unwrap();

    assert!(
        status.is_success(),
        "expected fallback after 404, got {status}"
    );
    assert!(serde_json::from_slice::<serde_json::Value>(&extract_body_bytes(resp_body)).is_ok());
}

#[tokio::test]
async fn openai_client_prefers_openai_upstream_in_mixed_group() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let anthropic_hits = Arc::new(AtomicUsize::new(0));
    let anthropic_hits_capture = anthropic_hits.clone();
    async fn anthropic_handler(hits: Arc<AtomicUsize>) -> Json<serde_json::Value> {
        hits.fetch_add(1, Ordering::SeqCst);
        Json(serde_json::json!({
            "id": "msg_mock",
            "type": "message",
            "role": "assistant",
            "model": "claude-mock",
            "content": [{"type": "text", "text": "anthropic"}],
            "stop_reason": "end_turn"
        }))
    }

    let anthropic_app = Router::new().route(
        "/v1/messages",
        post({
            let hits = anthropic_hits_capture.clone();
            move || anthropic_handler(hits.clone())
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let anthropic_addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, anthropic_app).await.ok(); });
    let anthropic_base = format!("http://{anthropic_addr}/anthropic");

    let openai = spawn_mock_upstream(false).await;

    let mut groups = HashMap::new();
    groups.insert(
        "high".to_string(),
        vec![
            ModelEndpoint {
                id: "anthropic-first".into(),
                base_url: anthropic_base,
                model: "claude-mock".into(),
                api_key: Some("k".into()),
                api_key_env: None,
                timeout_secs: 10,
                protocol: None,
            },
            ModelEndpoint {
                id: "openai-second".into(),
                base_url: openai,
                model: "mock-model".into(),
                api_key: Some("k".into()),
                api_key_env: None,
                timeout_secs: 10,
                protocol: None,
            },
        ],
    );

    let mut config = test_config("http://127.0.0.1:9");
    config.fallback_groups = groups;
    let (_app, proxy) = make_app(config);

    let body = serde_json::json!({
        "model": "saferoute-high",
        "messages": [{"role": "user", "content": "hello"}]
    });
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(axum::http::header::CONTENT_TYPE, "application/json".parse().unwrap());

    let (status, _, _) = proxy
        .handle_api_request(ProxyRequest {
            session_id: "sess-protocol-order",
            fallback_group: Some("high"),
            method: Method::POST,
            path: "/v1/chat/completions",
            query: None,
            headers,
            body: Bytes::from(serde_json::to_vec(&body).unwrap()),
        })
        .await
        .unwrap();

    assert!(status.is_success());
    assert_eq!(
        anthropic_hits.load(Ordering::SeqCst),
        0,
        "OpenAI client should hit OpenAI-native upstream first"
    );
}

#[tokio::test]
async fn file_session_dlp_via_proxy() {
    let tmp = tempfile::TempDir::new().unwrap();
    let secret = "P".repeat(65);
    std::fs::write(tmp.path().join("probe.txt"), &secret).unwrap();

    let recorded = Arc::new(Mutex::new(Vec::<u8>::new()));
    let recorded_capture = recorded.clone();
    async fn handler(
        recorded: Arc<Mutex<Vec<u8>>>,
        axum::Json(body): axum::Json<serde_json::Value>,
    ) -> Json<serde_json::Value> {
        *recorded.lock().unwrap() = serde_json::to_vec(&body).unwrap_or_default();
        Json(serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": "ok"}}]
        }))
    }

    let app = Router::new().route(
        "/v1/chat/completions",
        post({
            let recorded = recorded_capture.clone();
            move |body| handler(recorded, body)
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.ok(); });
    let upstream = format!("http://{addr}");

    let mut config = test_config(&upstream);
    config.file_rules = vec![FileRule {
        id: "file-session".into(),
        path: tmp.path().to_path_buf(),
        enabled: true,
        recursive: true,
        trigger_window: 5,
        match_mode: MatchMode::Full,
        min_fragment_len: None,
        min_fragment_ratio: None,
        formats: vec!["txt".into()],
        index: FileIndexOptions::default(),
    }];

    let (app, proxy) = make_app(config);
    for _ in 0..300 {
        if app.snapshot().dlp.is_file_index_ready() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(app.snapshot().dlp.is_file_index_ready());

    let session = "proxy-file-session";
    let probe_path = tmp
        .path()
        .join("probe.txt")
        .to_string_lossy()
        .replace('\\', "/");
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(axum::http::header::CONTENT_TYPE, "application/json".parse().unwrap());

    let trigger = serde_json::json!({
        "model": "mock-model",
        "messages": [
            {"role": "user", "content": "read probe"},
            {"role": "assistant", "content": null, "tool_calls": [{
                "id": "c1",
                "type": "function",
                "function": {
                    "name": "read_file",
                    "arguments": format!(r#"{{"path":"{probe_path}"}}"#)
                }
            }]}
        ],
        "max_tokens": 8
    });
    proxy
        .handle_api_request(ProxyRequest {
            session_id: session,
            fallback_group: Some("high"),
            method: Method::POST,
            path: "/v1/chat/completions",
            query: None,
            headers: headers.clone(),
            body: Bytes::from(serde_json::to_vec(&trigger).unwrap()),
        })
        .await
        .unwrap();

    let leak = serde_json::json!({
        "model": "mock-model",
        "messages": [{"role": "user", "content": format!("secret: {secret}")}],
        "max_tokens": 16
    });
    proxy
        .handle_api_request(ProxyRequest {
            session_id: session,
            fallback_group: Some("high"),
            method: Method::POST,
            path: "/v1/chat/completions",
            query: None,
            headers,
            body: Bytes::from(serde_json::to_vec(&leak).unwrap()),
        })
        .await
        .unwrap();

    let forward = recorded.lock().unwrap().clone();
    assert!(!forward.is_empty(), "upstream should receive leak request body");
    let forward_json = parse_json_body(&forward).unwrap();
    let content = forward_json["messages"][0]["content"].as_str().unwrap_or("");
    assert!(
        !content.contains(&secret),
        "forwarded request must not contain file secret; got {content}"
    );
}

#[tokio::test]
async fn dlp_and_route_work() {
    let upstream = spawn_mock_upstream(false).await;
    let (_app, proxy) = make_app(test_config(&upstream));

    let body = serde_json::json!({
        "model": "mock-model",
        "messages": [{"role": "user", "content": "key TOP-SECRET-KEY end"}]
    });
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(axum::http::header::CONTENT_TYPE, "application/json".parse().unwrap());

    let (status, _, resp_body) = proxy
        .handle_api_request(ProxyRequest {
            session_id: "sess-1",
            fallback_group: Some("high"),
            method: Method::POST,
            path: "/v1/chat/completions",
            query: None,
            headers,
            body: Bytes::from(serde_json::to_vec(&body).unwrap()),
        })
        .await
        .unwrap();

    assert!(status.is_success());
    assert!(serde_json::from_slice::<serde_json::Value>(&extract_body_bytes(resp_body)).is_ok());
}

#[tokio::test]
async fn operation_security_blocks_response() {
    let upstream = spawn_mock_upstream(true).await;
    let (_app, proxy) = make_app(test_config(&upstream));

    let body = serde_json::json!({
        "model": "mock-model",
        "messages": [{"role": "user", "content": "cleanup"}]
    });
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(axum::http::header::CONTENT_TYPE, "application/json".parse().unwrap());

    let (_, _, resp_body) = proxy
        .handle_api_request(ProxyRequest {
            session_id: "sess-2",
            fallback_group: Some("high"),
            method: Method::POST,
            path: "/v1/chat/completions",
            query: None,
            headers,
            body: Bytes::from(serde_json::to_vec(&body).unwrap()),
        })
        .await
        .unwrap();

    let resp_json = parse_json_body(&extract_body_bytes(resp_body)).unwrap();
    let texts = extract_texts(&resp_json).unwrap();
    let combined: String = texts.iter().map(|t| t.text.as_str()).collect();
    assert!(combined.contains("SMR BLOCKED"));
}

fn extract_body_bytes(body: smr_core::request::ProxyBody) -> Bytes {
    match body {
        smr_core::request::ProxyBody::Buffered(b) => b,
        smr_core::request::ProxyBody::SseStream(_) => {
            panic!("expected buffered response in test")
        }
    }
}

#[test]
fn config_loads_example_yaml() {
    let path = workspace_root().join("config/smr.example.yaml");
    let config = AppConfig::load(&path).unwrap();
    assert_eq!(config.server.listen, "127.0.0.1:8080");
    assert!(config.fallback_groups.contains_key("high"));
}

#[test]
fn config_validation_rejects_empty_groups() {
    let mut config = AppConfig {
        server: ServerConfig::default(),
        pipeline: PipelineConfig::default(),
        logging: LoggingConfig::default(),
        fallback_groups: HashMap::new(),
        content_rules: vec![],
        file_rules: vec![],
        operation_rules: vec![],
        path_protection_rules: vec![],
    };
    assert!(config.validate().is_err());
    config.fallback_groups.insert("high".into(), vec![]);
    assert!(config.validate().is_err());
}

#[tokio::test]
async fn health_and_ui_endpoints() {
    use smr_core::run_app;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    let mut config = test_config("http://127.0.0.1:9");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    config.server.listen = format!("127.0.0.1:{port}");

    let mut tmp = NamedTempFile::new().unwrap();
    write!(tmp, "{}", serde_yaml::to_string(&config).unwrap()).unwrap();
    let app = SharedApp::new(
        tmp.path().to_path_buf(),
        config,
        EventLog::new(50),
        Arc::new(AuditStore::open(&std::env::temp_dir().join("smr-test-db2")).unwrap()),
    )
    .unwrap();

    let handle = tokio::spawn(async move { run_app(app).await.ok(); });
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut buf = vec![0u8; 512];
    let n = stream.read(&mut buf).await.unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).contains("200"));

    let mut stream2 = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
    stream2
        .write_all(b"GET /ui HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut buf2 = vec![0u8; 2048];
    let n2 = stream2.read(&mut buf2).await.unwrap();
    assert!(String::from_utf8_lossy(&buf2[..n2]).contains("LLM-SafeRoute"));

    handle.abort();
}

#[tokio::test]
async fn provider_exposes_models_list() {
    let upstream = spawn_mock_upstream(false).await;
    let mut config = test_config(&upstream);
    config.fallback_groups.insert(
        "medium".into(),
        vec![ModelEndpoint {
            id: "mock-medium".into(),
            base_url: upstream.clone(),
            model: "mock-medium".into(),
            api_key: Some("test-key".into()),
            api_key_env: None,
            timeout_secs: 10,
            protocol: None,
        }],
    );
    config.fallback_groups.insert(
        "low".into(),
        vec![ModelEndpoint {
            id: "mock-low".into(),
            base_url: upstream,
            model: "mock-low".into(),
            api_key: Some("test-key".into()),
            api_key_env: None,
            timeout_secs: 10,
            protocol: None,
        }],
    );

    let (_app, proxy) = make_app(config);

    let (status, _, resp_body) = proxy
        .handle_api_request(ProxyRequest {
            session_id: "sess-models",
            fallback_group: None,
            method: Method::GET,
            path: "/v1/models",
            query: None,
            headers: axum::http::HeaderMap::new(),
            body: Bytes::new(),
        })
        .await
        .unwrap();

    assert!(status.is_success());
    let json: serde_json::Value =
        serde_json::from_slice(&extract_body_bytes(resp_body)).unwrap();
    let ids: Vec<_> = json["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m.get("id").and_then(|v| v.as_str()))
        .collect();
    assert!(ids.contains(&"saferoute-high"));
    assert!(ids.contains(&"saferoute-medium"));
    assert!(ids.contains(&"saferoute-lite"));
}

#[tokio::test]
async fn provider_model_id_selects_fallback_group() {
    let upstream = spawn_mock_upstream(false).await;
    let mut config = test_config(&upstream);
    config.fallback_groups.insert(
        "medium".into(),
        vec![ModelEndpoint {
            id: "mock-medium".into(),
            base_url: upstream,
            model: "mock-medium".into(),
            api_key: Some("test-key".into()),
            api_key_env: None,
            timeout_secs: 10,
            protocol: None,
        }],
    );

    let (app, proxy) = make_app(config);

    let body = serde_json::json!({
        "model": "saferoute-medium",
        "messages": [{"role": "user", "content": "hello"}]
    });
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(axum::http::header::CONTENT_TYPE, "application/json".parse().unwrap());

    let (status, _, _) = proxy
        .handle_api_request(ProxyRequest {
            session_id: "sess-model-route",
            fallback_group: None,
            method: Method::POST,
            path: "/v1/chat/completions",
            query: None,
            headers,
            body: Bytes::from(serde_json::to_vec(&body).unwrap()),
        })
        .await
        .unwrap();

    assert!(status.is_success());
    let audits = app.storage.list_audits(20).unwrap();
    let audit = audits
        .iter()
        .find(|a| a.session_id == "sess-model-route")
        .expect("audit row");
    assert_eq!(audit.fallback_group, "medium");
}

#[tokio::test]
async fn universal_api_messages_path_alias() {
    let upstream = spawn_mock_upstream(false).await;
    let config = test_config(&upstream);
    let (app, proxy) = make_app(config);

    let body = serde_json::json!({
        "model": "saferoute-high",
        "max_tokens": 32,
        "messages": [{"role": "user", "content": "hello"}]
    });
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(axum::http::header::CONTENT_TYPE, "application/json".parse().unwrap());

    let (status, _, _) = proxy
        .handle_api_request(ProxyRequest {
            session_id: "sess-universal-messages",
            fallback_group: None,
            method: Method::POST,
            path: "/messages",
            query: None,
            headers,
            body: Bytes::from(serde_json::to_vec(&body).unwrap()),
        })
        .await
        .unwrap();

    assert!(status.is_success());
    let audits = app.storage.list_audits(20).unwrap();
    let audit = audits
        .iter()
        .find(|a| a.session_id == "sess-universal-messages")
        .expect("audit row");
    assert_eq!(audit.fallback_group, "high");
    assert_eq!(audit.protocol, "Anthropic");
}

async fn spawn_mock_anthropic_upstream() -> String {
    async fn handler() -> Json<serde_json::Value> {
        Json(serde_json::json!({
            "id": "msg_mock",
            "type": "message",
            "role": "assistant",
            "model": "claude-mock",
            "content": [{"type": "text", "text": "hi from anthropic"}],
            "stop_reason": "end_turn"
        }))
    }

    let app = Router::new().route("/v1/messages", post(handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.ok(); });
    format!("http://{addr}/v1")
}

#[tokio::test]
async fn universal_api_converts_openai_client_to_anthropic_upstream() {
    let upstream = spawn_mock_anthropic_upstream().await;
    let mut config = test_config(&upstream);
    config.fallback_groups.get_mut("high").unwrap()[0].protocol = Some("anthropic".into());
    config.fallback_groups.get_mut("high").unwrap()[0].model = "claude-mock".into();

    let (app, proxy) = make_app(config);

    let body = serde_json::json!({
        "model": "saferoute-high",
        "messages": [{"role": "user", "content": "hello"}]
    });
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(axum::http::header::CONTENT_TYPE, "application/json".parse().unwrap());

    let (status, resp_headers, resp_body) = proxy
        .handle_api_request(ProxyRequest {
            session_id: "sess-universal-convert",
            fallback_group: None,
            method: Method::POST,
            path: "/v1/chat/completions",
            query: None,
            headers,
            body: Bytes::from(serde_json::to_vec(&body).unwrap()),
        })
        .await
        .unwrap();

    assert!(status.is_success());
    let audits = app.storage.list_audits(20).unwrap();
    let audit = audits
        .iter()
        .find(|a| a.session_id == "sess-universal-convert")
        .expect("audit row");
    assert_eq!(audit.protocol, "OpenAI");
    let json: serde_json::Value = serde_json::from_slice(&extract_body_bytes(resp_body)).unwrap();
    assert!(json.get("choices").is_some());
    assert!(
        json["choices"][0]["message"]["content"]
            .as_str()
            .is_some_and(|s| s.contains("anthropic"))
    );
    assert!(resp_headers.get("content-type").is_some());
}
