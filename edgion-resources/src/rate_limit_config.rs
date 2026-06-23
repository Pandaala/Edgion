//! Global RateLimit configuration (CMS estimator slots + gateway instance count).
//!
//! Lives in `core::common` so the RateLimit resource type
//! (`types::resources::edgion_plugins::plugin_configs::rate_limit`) can read these
//! globals during validation/compilation without depending on `core::gateway`.
//! Initialized once at gateway startup via [`init_rate_limit_global_config`].

use serde::{Deserialize, Serialize};

/// Unit multiplier: 1K = 1024 slots
pub const SLOTS_K: usize = 1024;

/// Minimum CMS estimator slots in K (1K = 1024 slots, ~64KB)
pub const MIN_ESTIMATOR_SLOTS_K: usize = 1;

/// Default CMS estimator slots in K (64K = 65536 slots, ~4MB)
pub const DEFAULT_ESTIMATOR_SLOTS_K: usize = 64;

/// Maximum CMS estimator slots in K (1024K = 1M slots, ~64MB)
pub const DEFAULT_MAX_ESTIMATOR_SLOTS_K: usize = 1024;

/// RateLimit plugin global configuration
///
/// Controls the Count-Min Sketch (CMS) estimator settings for all RateLimit plugins.
/// The CMS algorithm provides memory-efficient rate limiting with configurable precision.
///
/// All slot values are in K units (1K = 1024 slots).
/// Memory per Rate instance ≈ slots_k × 64KB
///
/// ## YAML Configuration Example:
/// ```yaml
/// rate_limit:
///   default_estimator_slots_k: 64     # 64K slots = ~4MB
///   max_estimator_slots_k: 1024       # 1024K slots = ~64MB
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RateLimitGlobalConfig {
    /// Default CMS estimator slots in K units (default: 64)
    ///
    /// Controls the precision and memory usage of the rate limiting algorithm.
    /// Higher values reduce hash collisions but increase memory usage.
    /// Memory per Rate instance ≈ slots_k × 64KB
    ///
    /// | Value | Slots | Memory | Use Case |
    /// |-------|-------|--------|----------|
    /// | 1     | 1K    | 64KB   | Minimum  |
    /// | 8     | 8K    | 512KB  | Low cardinality |
    /// | 64    | 64K   | 4MB    | Default, most scenarios |
    /// | 256   | 256K  | 16MB   | High cardinality |
    /// | 1024  | 1M    | 64MB   | Maximum precision |
    #[serde(default = "default_estimator_slots_k")]
    pub default_estimator_slots_k: usize,

    /// Maximum allowed CMS estimator slots in K units (default: 1024)
    ///
    /// Limits the maximum slots that can be configured per RateLimit plugin.
    /// Maximum memory per Rate instance ≈ max_slots_k × 64KB
    #[serde(default = "default_max_estimator_slots_k")]
    pub max_estimator_slots_k: usize,

    /// Static fallback for gateway instance count (default: 1)
    ///
    /// Used when controller is not available to push dynamic count.
    /// Set this to your expected replica count for best-effort
    /// cluster rate limiting without controller.
    #[serde(default = "default_gateway_instance_count")]
    pub gateway_instance_count: u32,
}

fn default_estimator_slots_k() -> usize {
    DEFAULT_ESTIMATOR_SLOTS_K
}

fn default_max_estimator_slots_k() -> usize {
    DEFAULT_MAX_ESTIMATOR_SLOTS_K
}

fn default_gateway_instance_count() -> u32 {
    1
}

impl Default for RateLimitGlobalConfig {
    fn default() -> Self {
        Self {
            default_estimator_slots_k: DEFAULT_ESTIMATOR_SLOTS_K,
            max_estimator_slots_k: DEFAULT_MAX_ESTIMATOR_SLOTS_K,
            gateway_instance_count: default_gateway_instance_count(),
        }
    }
}

// ============================================
// Global RateLimit Config Store
// ============================================

use parking_lot::RwLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::LazyLock;

// ============================================
// Gateway Instance Count (for Cluster scope)
// ============================================

/// Global gateway instance count.
///
/// Updated by the WatchServerMeta background task (control plane).
/// Read by RateLimit plugins on every request (data plane hot path).
///
/// Default is 1: when controller is unavailable, Cluster scope
/// degrades to Instance scope behavior (with skewTolerance applied).
static GATEWAY_INSTANCE_COUNT: AtomicU32 = AtomicU32::new(1);

/// Update the gateway instance count (called by control plane)
pub fn set_gateway_instance_count(count: u32) {
    let count = count.max(1); // Prevent division by zero
    let old = GATEWAY_INSTANCE_COUNT.swap(count, Ordering::Relaxed);
    if old != count {
        tracing::info!(old_count = old, new_count = count, "Gateway instance count updated");
    }
}

/// Get the current gateway instance count (called by data plane hot path)
///
/// Cost: ~0.3ns on x86 (single MOV instruction, no memory fence)
#[inline]
pub fn get_gateway_instance_count() -> u32 {
    GATEWAY_INSTANCE_COUNT.load(Ordering::Relaxed)
}

/// Global store for RateLimit configuration
static RATE_LIMIT_GLOBAL_CONFIG: LazyLock<RwLock<RateLimitGlobalConfig>> =
    LazyLock::new(|| RwLock::new(RateLimitGlobalConfig::default()));

/// Initialize the global RateLimit configuration
///
/// This should be called once during application startup with the loaded config.
pub fn init_rate_limit_global_config(config: &RateLimitGlobalConfig) {
    let mut global = RATE_LIMIT_GLOBAL_CONFIG.write();
    *global = config.clone();

    // Initialize instance count from static config
    // (may be overridden by WatchServerMeta later)
    set_gateway_instance_count(config.gateway_instance_count);

    tracing::info!(
        default_slots_k = config.default_estimator_slots_k,
        max_slots_k = config.max_estimator_slots_k,
        default_slots = config.default_estimator_slots_k * SLOTS_K,
        max_slots = config.max_estimator_slots_k * SLOTS_K,
        gateway_instance_count = config.gateway_instance_count,
        "RateLimit global config initialized"
    );
}

/// Get the default estimator slots from global config (returns actual slot count)
pub fn get_default_estimator_slots() -> usize {
    RATE_LIMIT_GLOBAL_CONFIG.read().default_estimator_slots_k * SLOTS_K
}

/// Get the maximum estimator slots from global config (returns actual slot count)
pub fn get_max_estimator_slots() -> usize {
    RATE_LIMIT_GLOBAL_CONFIG.read().max_estimator_slots_k * SLOTS_K
}

/// Get the minimum estimator slots (returns actual slot count)
pub fn get_min_estimator_slots() -> usize {
    MIN_ESTIMATOR_SLOTS_K * SLOTS_K
}
