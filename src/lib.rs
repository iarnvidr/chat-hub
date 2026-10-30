pub mod archive;
pub mod ca;
pub mod capture;
pub mod capture_lane;
pub mod config;
pub mod error;
pub mod importer;
pub mod library_export;
pub mod live_capture;
pub mod proxy;
pub mod runtime;
pub mod server;
pub mod service;
pub mod store;

// Proxy integration contract: proxy::run(ProxyConfig) -> Result<()>; kept unreferenced until available.
