//! EWMA alpha (smoothing factor) configuration.
//!
//! Service-scoped EWMA latency tracking now lives in `runtime_state`.
//! This module only owns the global alpha parameter shared by all services.

use std::sync::atomic::{AtomicU32, Ordering};

/// Alpha parameter for EWMA calculation (0-100, representing 0.0-1.0)
/// Default: 20 (represents 0.2)
static ALPHA_PERCENT: AtomicU32 = AtomicU32::new(20);

/// Set alpha parameter (smoothing factor) for EWMA calculation.
///
/// # Arguments
/// * `alpha` - Alpha value as percentage (0-100), where 20 = 0.2
///
/// Higher values = more weight on recent samples, more responsive to changes.
/// Lower values  = more smoothing, less sensitive to spikes.
pub fn set_alpha(alpha: u32) {
    let clamped = alpha.clamp(0, 100);
    ALPHA_PERCENT.store(clamped, Ordering::Relaxed);
    tracing::info!(alpha = clamped, "EWMA alpha parameter updated");
}

/// Get current alpha parameter.
pub fn get_alpha() -> u32 {
    ALPHA_PERCENT.load(Ordering::Relaxed)
}
