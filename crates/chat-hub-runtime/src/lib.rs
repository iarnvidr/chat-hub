//! Embedded service lifecycle and composition shared by native desktop and CLI.
pub use chat_hub_api::server;
pub use chat_hub_capture::{capture_lane, live_capture, operation_learner, proxy};
pub use chat_hub_store::{error, store};
pub mod config;
pub mod runtime;
pub mod service;
