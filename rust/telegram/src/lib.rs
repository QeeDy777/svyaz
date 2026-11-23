//! Loopback MTProto server for «Связь». Wire compatibility is based on Flowseal tg-ws-proxy 1.11.4.
mod config;
mod fake_tls;
mod protocol;
mod runner;
mod runtime_config;
mod transport;
mod transport_h2;

pub use config::{ensure_secret, proxy_url, proxy_url_at, validate};
pub use runner::{EngineState, Runner};
pub const VERSION: &str = "1.11.4";
