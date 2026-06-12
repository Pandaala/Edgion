//! Gateway logging subdomain: system log and protocol-specific log sinks

pub mod buffer;
pub mod logger_factory;
pub mod ssl_log;
pub mod tcp_log;
pub mod tls_log;
pub mod udp_log;

pub use crate::core::gateway::link_sys::providers::local_file::LogType;
// System log bootstrap now lives in `common::observe::logs`; re-export it here
// so existing `gateway::observe::logs::{SysLogConfig, ...}` call sites are stable.
pub use crate::core::common::observe::{init_default, init_logging, SysLogConfig};
pub use buffer::{LogBuffer, ULogBuffer};
pub use logger_factory::{create_async_logger, create_sync_logger};
pub use ssl_log::{init_ssl_logger, log_ssl};
pub use tcp_log::{init_tcp_logger, log_tcp, TcpLogEntry};
pub use tls_log::{init_tls_logger, log_tls};
pub use udp_log::{init_udp_logger, log_udp, UdpLogEntry};
