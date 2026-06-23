//! Kubernetes backend configuration.
//!
//! The schema definitions live in `crate::config::conf_center::kubernetes`.
//! Re-exported here to keep existing controller call sites stable.

pub use crate::config::conf_center::kubernetes::{
    HaMode, KubernetesConfig, LeaderElectionConfig, MetadataFilterConfig,
};
