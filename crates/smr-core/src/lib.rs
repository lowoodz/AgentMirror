pub mod admin;
pub mod audit;
pub mod body_util;
pub mod config;
pub mod dlp;
pub mod events;
pub mod http_state;
pub mod ops;
pub mod paths;
pub mod proxy;
pub mod request;
pub mod router;
pub mod server;
pub mod sse_stream;
pub mod state;
pub mod storage;
pub mod streaming;
pub mod traffic;

pub use config::AppConfig;
pub use server::run_app;
pub use state::SharedApp;

pub const DEFAULT_CONFIG_YAML: &str = include_str!("../../../config/smr.example.yaml");
