use std::sync::OnceLock;

pub use crate::core::controller::conf_mgr::conf_center::EndpointMode;

/// Global endpoint mode - initialized once at startup, zero-cost read access.
static GLOBAL_ENDPOINT_MODE: OnceLock<EndpointMode> = OnceLock::new();

/// Initialize global endpoint mode (called once at startup).
///
/// Uses get_or_init to avoid panics on restart and logs mismatch if detected.
pub fn init_global_endpoint_mode(mode: EndpointMode) {
    let existing = GLOBAL_ENDPOINT_MODE.get_or_init(|| {
        tracing::info!(
            component = "backends",
            mode = ?mode,
            "Global endpoint mode initialized"
        );
        mode
    });

    if *existing != mode {
        tracing::warn!(
            component = "backends",
            existing_mode = ?existing,
            new_mode = ?mode,
            "Endpoint mode mismatch on restart - using existing mode"
        );
    }
}

/// Get global endpoint mode (zero-cost after initialization).
#[inline]
pub fn get_global_endpoint_mode() -> EndpointMode {
    *GLOBAL_ENDPOINT_MODE
        .get()
        .expect("GLOBAL_ENDPOINT_MODE not initialized - call init_global_endpoint_mode first")
}

/// Try to get global endpoint mode without panicking.
///
/// Returns `None` if the endpoint mode has not been initialized yet.
/// Useful for code that needs to check endpoint mode but may run before initialization.
#[inline]
pub fn try_get_global_endpoint_mode() -> Option<EndpointMode> {
    GLOBAL_ENDPOINT_MODE.get().copied()
}

/// Check if using EndpointSlice mode.
#[inline]
pub fn is_endpoint_slice_mode() -> bool {
    matches!(get_global_endpoint_mode(), EndpointMode::EndpointSlice)
}
