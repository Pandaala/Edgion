//! Etcd-based configuration center backend
//!
//! Stores and watches Edgion configuration resources (Gateway, Route, Plugin, etc.)
//! in an etcd cluster. Reuses `EtcdLinkClient` from the common LinkSys core
//! (`edgion_common::link_sys_core::providers::etcd`, also re-exported by the
//! gateway) for connection management, health monitoring, and KV/Watch operations.
//!
//! ## Architecture
//!
//! ```text
//! EtcdCenter (CenterLifeCycle)
//! ├── EtcdStorage (CenterApi) → EtcdLinkClient
//! ├── EtcdController (2-phase spawn)
//! │   ├── EtcdWatcher (init + runtime watch)
//! │   └── EtcdResourceController × N
//! └── EtcdStatusHandler (/status/ prefix)
//! ```

pub mod center;
pub mod config;
pub mod controller;
pub mod event;
pub mod resource_controller;
pub mod status;
pub mod storage;
pub mod watcher;

pub use center::EtcdCenter;
pub use config::EtcdConfig;
