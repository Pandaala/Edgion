//! Gateway logging subdomain: system log and protocol-specific log sinks

pub mod buffer;
pub mod logger_factory;
pub mod ssl_log;
pub mod sys_log;
pub mod tcp_log;
pub mod tls_log;
pub mod udp_log;

pub use buffer::{LogBuffer, ULogBuffer};
pub use logger_factory::{create_async_logger, create_sync_logger};
pub use ssl_log::{init_ssl_logger, log_ssl};
pub use sys_log::{init_default, init_logging, LogConfig as SysLogConfig};
pub use tcp_log::{init_tcp_logger, log_tcp, TcpLogEntry};
pub use tls_log::{init_tls_logger, log_tls, TlsLogEntry};
pub use udp_log::{init_udp_logger, log_udp, UdpLogEntry};
