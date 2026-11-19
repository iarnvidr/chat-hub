//! Bounded, database-independent network capture.
//!
//! The runtime supplies an [`chat_hub_model::ObservationSink`] to complete queued
//! captures. Parsing, backpressure, accounting and proxy transport live here;
//! database writes and service lifecycle remain outside this crate.

pub mod ca;
pub mod capture_lane;
pub mod live_capture;
pub mod operation_learner;
pub mod proxy;
mod request_identity;
