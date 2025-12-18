/// Global definitions and constants for Edgion

use std::path::PathBuf;
use std::sync::LazyLock;

// Component names
pub const COMPONENT_EDGION_CONTROLLER: &str = "edgion-controller";
pub const COMPONENT_EDGION_GATEWAY: &str = "edgion-gateway";

// Log file prefixes
pub const LOG_PREFIX_OPERATOR: &str = "edgion-operator";
pub const LOG_PREFIX_GATEWAY: &str = "edgion-gateway";

// Default directories
pub const DEFAULT_LOG_DIR: &str = "logs";
pub const DEFAULT_CONFIG_DIR: &str = "examples/conf";
pub const DEFAULT_PREFIX_DIR: &str = "/usr/local/edgion";

// Version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

// Global prefix directory
static PREFIX_DIR: LazyLock<PathBuf> = LazyLock::new(|| {
    std::env::var("EDGION_PREFIX_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/usr/local/edgion"))
});

/// Get the global prefix directory
pub fn prefix_dir() -> &'static PathBuf {
    &PREFIX_DIR
}
