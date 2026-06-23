//! Configuration Center schema types (backend-agnostic).
//!
//! These are pure serde/schema definitions for the configuration center. They
//! live in the controller (`crate::config::conf_center`) alongside
//! `EdgionControllerConfig`, which embeds them; the gateway never reads them.
//!
//! The controller's `conf_mgr::conf_center` runtime modules re-export these
//! types, so existing call sites are unaffected. Backend-specific runtime
//! behavior (centers, controllers, storage) lives in those `conf_mgr` modules.

pub mod config;
pub mod etcd;
pub mod file_system;
pub mod kubernetes;

pub use config::ConfCenterConfig;
pub use etcd::{EtcdAuthConfig, EtcdConfig, EtcdTlsConfig};
pub use file_system::FileSystemConfig;
pub use kubernetes::{HaMode, KubernetesConfig, LeaderElectionConfig, MetadataFilterConfig};
