//! Edgion end-to-end test harness — shared library.
//!
//! This crate hosts the ~56k-line black-box integration test framework that
//! drives a running Edgion gateway over the wire (HTTP/gRPC/TCP/UDP/WS/TLS).
//!
//! The shared client framework lives here as library modules; the executable
//! entrypoints (`test_client`, `test_server`, `test_client_direct`, the
//! validators, and `build_test_proto`) are declared as `[[bin]]` targets in
//! `Cargo.toml` and consume these modules via `edgion_tests::…`.
//!
//! The module files physically live under `src/client/` (next to the
//! `test_client` binary) and are re-rooted here via `#[path]` so the existing
//! intra-framework `crate::framework` / `crate::suites` / `crate::dynamic`
//! references keep resolving unchanged.

#![allow(clippy::module_inception)]
#![allow(dead_code)]
#![allow(unused_imports)]

#[path = "client/access_log_client.rs"]
pub mod access_log_client;
#[path = "client/dynamic/mod.rs"]
pub mod dynamic;
#[path = "client/framework.rs"]
pub mod framework;
#[path = "client/log_analyzer.rs"]
pub mod log_analyzer;
#[path = "client/metrics_helper.rs"]
pub mod metrics_helper;
#[path = "client/port_config.rs"]
pub mod port_config;
#[path = "client/reporter.rs"]
pub mod reporter;
#[path = "client/suites/mod.rs"]
pub mod suites;
#[path = "client/transport.rs"]
pub mod transport;
