//! Kubernetes backend configuration.
//!
//! The schema definitions now live in `common::config::conf_center::kubernetes`.
//! Re-exported here to keep existing controller call sites stable.

pub use crate::core::common::config::conf_center::kubernetes::{
    HaMode, KubernetesConfig, LeaderElectionConfig, MetadataFilterConfig,
};
