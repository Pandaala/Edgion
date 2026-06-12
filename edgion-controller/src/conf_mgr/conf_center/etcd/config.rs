//! Etcd ConfCenter configuration.
//!
//! The schema types and the `EtcdConfig::to_client_config` conversion now live
//! in `edgion_common::config::conf_center::etcd`. Re-exported here to keep
//! existing controller call sites (`super::config::EtcdConfig`, `etcd::EtcdConfig`)
//! stable.

pub use edgion_common::config::conf_center::etcd::{EtcdAuthConfig, EtcdConfig, EtcdTlsConfig};
