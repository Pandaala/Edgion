//! Application identity and global constants

/// Component name for Edgion Controller
pub const CONTROLLER_NAME: &str = "edgion-controller";

/// Component name for Edgion Gateway
pub const GATEWAY_NAME: &str = "edgion-gateway";

/// Log file prefix for controller/operator logs
pub const LOG_PREFIX_OPERATOR: &str = "edgion-operator";

/// Log file prefix for gateway logs
pub const LOG_PREFIX_GATEWAY: &str = "edgion-gateway";

/// Default log directory
pub const DEFAULT_LOG_DIR: &str = "logs";

/// Default configuration directory
pub const DEFAULT_CONFIG_DIR: &str = "examples/conf";

/// Default working directory
pub const DEFAULT_WORK_DIR: &str = ".";

/// Application version (from Cargo.toml)
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

// Backward compatibility aliases
pub use CONTROLLER_NAME as COMPONENT_EDGION_CONTROLLER;
pub use GATEWAY_NAME as COMPONENT_EDGION_GATEWAY;
