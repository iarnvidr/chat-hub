//! Stable public facade and CLI composition for the modular Chat Hub workspace.
//!
//! Implementations live in their owning crates. These reexports preserve the
//! desktop and integration-test API without duplicating a second application core.
pub use chat_hub_api::server;
pub use chat_hub_capture::{ca, capture_lane, live_capture, proxy};
pub use chat_hub_ingest::importer;
pub use chat_hub_runtime::{config, runtime, service};
pub use chat_hub_store::{archive, artifacts, capture, catalog, error, library_export, store};
