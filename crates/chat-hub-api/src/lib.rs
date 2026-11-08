//! Authenticated loopback API. This adapter owns HTTP, not capture or storage internals.
pub use chat_hub_ingest::importer;
pub use chat_hub_store::{artifacts, catalog, error, store};
pub mod server;

// Capture status is a model contract, not a dependency on the proxy runtime.
pub mod live_capture {
    pub use chat_hub_model::{Counters, Snapshot};
}
