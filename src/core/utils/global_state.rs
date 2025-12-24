//! Global runtime state management

use std::sync::OnceLock;

static IS_K8S_MODE: OnceLock<bool> = OnceLock::new();

/// Set K8s mode (can only be set once during startup)
pub fn set_k8s_mode(is_k8s: bool) {
    IS_K8S_MODE.get_or_init(|| is_k8s);
}

/// Check if running in K8s mode
pub fn is_k8s_mode() -> bool {
    *IS_K8S_MODE.get_or_init(|| false)
}

/// Detect K8s mode with priority: CLI > Env > Config > Default(false)
pub fn detect_k8s_mode(
    cli_mode: Option<bool>,
    config_mode: Option<bool>,
) -> bool {
    // 1. CLI parameter (highest priority)
    if let Some(mode) = cli_mode {
        return mode;
    }
    
    // 2. Environment variable auto-detection
    if std::env::var("KUBERNETES_SERVICE_HOST").is_ok() {
        return true;
    }
    
    // 3. Configuration file
    if let Some(mode) = config_mode {
        return mode;
    }
    
    // 4. Default value
    false
}

