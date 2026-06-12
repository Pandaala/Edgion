pub mod elasticsearch;
pub mod local_file;
pub mod redis;
pub mod webhook;

// The etcd provider now lives in `common::link::providers::etcd` (it is the
// only provider the control plane needs). Re-export it from the original path.
pub use edgion_common::link::providers::etcd;

pub use elasticsearch::EsLinkClient;
pub use etcd::EtcdLinkClient;
pub use local_file::{LocalFileWriter, LogType};
pub use redis::RedisLinkClient;
