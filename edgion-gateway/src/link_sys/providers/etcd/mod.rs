//! Gateway Etcd LinkSys: re-export the shared client and the runner apply owner.

pub(crate) mod apply;

pub(crate) use apply::EtcdApply;
pub use edgion_common::link_sys_core::providers::etcd::*;
