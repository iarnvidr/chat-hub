//! SQLite persistence, migrations and durable archives for Chat Hub.
//!
//! No network, HTTP adapter, native UI or application-runtime dependency belongs
//! here. Provider normalization is supplied by the pure formats crate.
pub mod archive;
pub mod artifacts;
pub mod capture;
pub mod capture_journal;
pub mod catalog;
pub mod error;
pub mod library_export;
pub mod store;

pub use rusqlite::InterruptHandle;
