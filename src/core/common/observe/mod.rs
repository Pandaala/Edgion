//! Shared observability primitives.
//!
//! This module hosts process-wide helpers that are reused by multiple
//! Edgion binaries (Gateway, Controller, Center). The primary concern
//! today is Prometheus metrics exposition: installing the global
//! recorder once per process and rendering a `/metrics` endpoint.
//!
//! See `metrics_api` for the recorder helpers and Axum handler.

pub mod fed_metrics;
pub mod metrics_api;
pub mod system_metrics;
