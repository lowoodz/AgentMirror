use std::collections::HashMap;
use std::sync::Arc;

use axum::http::Method;
use axum::routing::post;
use axum::{Json, Router};
use bytes::Bytes;
use smr_core::config::*;
use smr_core::events::EventLog;
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
        }],
    );

    AppConfig {
        server: ServerConfig {
            listen: "127.0.0.1:0".into(),
            default_fallback_group: "high".into(),
        },
        pipeline: PipelineConfig {
            dlp_enabled: true,
            operation_security_mode: OperationSecurityMode::Enforce,
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
    }
}

fn make_app(config: AppConfig) -> (Arc<SharedApp>, ProxyService) {
    let mut tmp = NamedTempFile::new().unwrap();
    write!(tmp, "{}", serde_yaml::to_string(&config).unwrap()).unwrap();
    let app = SharedApp::new(tmp.path().to_path_buf(), config, EventLog::new(100)).unwrap();
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
    assert!(serde_json::from_slice::<serde_json::Value>(&resp_body).is_ok());
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

    let resp_json = parse_json_body(&resp_body).unwrap();
    let texts = extract_texts(&resp_json).unwrap();
    let combined: String = texts.iter().map(|t| t.text.as_str()).collect();
    assert!(combined.contains("SMR BLOCKED"));
}

#[test]
fn config_loads_example_yaml() {
    let path = workspace_root().join("config/smr.example.yaml");
    let config = AppConfig::load(&path).unwrap();
    assert_eq!(config.server.listen, "127.0.0.1:8080");
    assert!(config.fallback_groups.contains_key("high"));
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
    let app = SharedApp::new(tmp.path().to_path_buf(), config, EventLog::new(50)).unwrap();

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
    assert!(String::from_utf8_lossy(&buf2[..n2]).contains("SecureModelRoute"));

    handle.abort();
}
