pub mod admin;
pub mod audit;
pub mod body_util;
pub mod config;
pub mod dlp;
pub mod events;
pub mod http_state;
pub mod ops;
pub mod paths;
pub mod path_display;
pub mod proxy;
pub mod proxy_path;
pub mod provider;
pub mod request;
pub mod router;
pub mod server;
pub mod session_key;
pub mod sse_sanitize;
pub mod sse_stream;
pub mod state;
pub mod storage;
pub mod sse_tool_ops;
pub mod streaming;
pub mod tool_bundle;
pub mod traffic;
pub mod traffic_parse;

pub use config::AppConfig;
pub use server::run_app;
pub use state::SharedApp;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Changes when embedded admin UI (`assets/index.html`) changes.
pub const UI_DIGEST: &str = env!("SMR_UI_DIGEST");
pub const DEFAULT_CONFIG_YAML: &str = include_str!("../../../config/smr.example.yaml");
