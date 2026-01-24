//! ConfCenter configuration
//!
//! Re-exports configuration types from conf_mgr_new module.
//! This maintains backward compatibility during the migration period.

// Re-export all configuration types from new module
pub use crate::core::conf_mgr_new::conf_center::{
    ConfCenterConfig, EndpointMode, LeaderElectionConfig, MetadataFilterConfig,
};
