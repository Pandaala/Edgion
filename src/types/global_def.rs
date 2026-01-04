/// Global definitions and constants for Edgion


// Component names
pub const COMPONENT_EDGION_CONTROLLER: &str = "edgion-controller";
pub const COMPONENT_EDGION_GATEWAY: &str = "edgion-gateway";

// Log file prefixes
pub const LOG_PREFIX_OPERATOR: &str = "edgion-operator";
pub const LOG_PREFIX_GATEWAY: &str = "edgion-gateway";

// Default directories
pub const DEFAULT_LOG_DIR: &str = "logs";
pub const DEFAULT_CONFIG_DIR: &str = "examples/conf";
pub const DEFAULT_WORK_DIR: &str = ".";

// Version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
