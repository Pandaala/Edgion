/// Global definitions and constants for Edgion

use std::path::PathBuf;
use std::sync::OnceLock;

// Component names
pub const COMPONENT_EDGION_OPERATOR: &str = "edgion-operator";
pub const COMPONENT_EDGION_GATEWAY: &str = "edgion-gateway";

// Log file prefixes
pub const LOG_PREFIX_OPERATOR: &str = "edgion-operator";
pub const LOG_PREFIX_GATEWAY: &str = "edgion-gateway";

// Default directories
pub const DEFAULT_LOG_DIR: &str = "logs";
pub const DEFAULT_CONFIG_DIR: &str = "edgion/config/examples";
pub const DEFAULT_PREFIX_DIR: &str = "/usr/local/edgion";

// Version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

// Global prefix directory
static PREFIX_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Initialize and create the global prefix directory
pub fn init_prefix_dir(path: impl Into<PathBuf>) -> std::io::Result<&'static PathBuf> {
    let path = path.into();
    std::fs::create_dir_all(&path)?;
    let _ = PREFIX_DIR.set(path);
    Ok(PREFIX_DIR.get().unwrap())
}

/// Get the global prefix directory (must call init_prefix_dir first)
pub fn prefix_dir() -> &'static PathBuf {
    PREFIX_DIR.get().expect("prefix_dir not initialized")
}
