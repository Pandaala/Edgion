//! Application identity and global constants

/// Component name for Edgion Controller
pub const CONTROLLER_NAME: &str = "edgion-controller";

/// Component name for Edgion Gateway
pub const GATEWAY_NAME: &str = "edgion-gateway";

/// Application version (from Cargo.toml)
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

// Backward compatibility alias
pub use CONTROLLER_NAME as COMPONENT_EDGION_CONTROLLER;
