//! Configuration Center schema types (backend-agnostic).
//!
//! These are pure serde/schema definitions for the configuration center,
//! moved down into `common` so that shared config loading
//! (`common::config::EdgionControllerConfig`) and helpers
//! (`common::utils::metadata_filter`) can reference them without reaching up
//! into the controller crate.
//!
//! The controller's `conf_mgr::conf_center` runtime modules re-export these
//! types from their original paths, so existing call sites are unaffected.
//! Backend-specific runtime behavior (centers, controllers, storage) and the
//! gateway-dependent `EtcdConfig::to_client_config` conversion stay in the
//! controller.

pub mod config;
pub mod etcd;
pub mod file_system;
pub mod kubernetes;

pub use config::ConfCenterConfig;
pub use etcd::{EtcdAuthConfig, EtcdConfig, EtcdTlsConfig};
pub use file_system::FileSystemConfig;
pub use kubernetes::{HaMode, KubernetesConfig, LeaderElectionConfig, MetadataFilterConfig};
