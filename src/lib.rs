pub mod ca;
pub mod capture;
pub mod capture_lane;
pub mod config;
pub mod error;
pub mod proxy;
pub mod runtime;
pub mod server;
pub mod service;
pub mod store;

// Proxy integration contract: proxy::run(ProxyConfig) -> Result<()>; kept unreferenced until available.
