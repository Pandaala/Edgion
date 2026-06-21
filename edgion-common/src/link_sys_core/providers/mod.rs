//! Link-system providers that live in `common`.
//!
//! Only the etcd provider lives here, because it is consumed by the control
//! plane. The remaining providers (webhook, redis, elasticsearch, local_file)
//! stay in `gateway::link_sys::providers`.

pub mod etcd;

pub use etcd::EtcdLinkClient;
