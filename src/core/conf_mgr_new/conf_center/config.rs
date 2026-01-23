//! ConfCenter configuration
//!
//! Re-exports configuration types from the original conf_mgr module
//! to maintain type compatibility across the codebase.

// Re-export all configuration types from old module to avoid type conflicts
// These types are shared between old and new architectures
pub use crate::core::conf_mgr::conf_center::{
    ConfCenterConfig, EndpointMode, LeaderElectionConfig, MetadataFilterConfig,
};
