pub mod elasticsearch;
pub mod kafka;
pub mod local_file;
pub mod otlp;
pub mod redis;
pub mod webhook;

// etcd provider lives in edgion_common; re-export it here for gateway consumers.
pub use edgion_common::link_sys_core::providers::etcd;

pub use elasticsearch::EsLinkClient;
pub use etcd::EtcdLinkClient;
pub use local_file::{LocalFileWriter, LogType};
pub use otlp::OtlpLogSender;
pub use redis::RedisLinkClient;
