//! Kubernetes backend configuration.
//!
//! The schema definitions now live in `common::config::conf_center::kubernetes`.
//! Re-exported here to keep existing controller call sites stable.

pub use edgion_common::config::conf_center::kubernetes::{
    HaMode, KubernetesConfig, LeaderElectionConfig, MetadataFilterConfig,
};
