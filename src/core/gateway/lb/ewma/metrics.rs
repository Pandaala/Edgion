//! EWMA (Exponentially Weighted Moving Average) metrics tracking
//!
//! Tracks response latency using EWMA algorithm for load balancing decisions.
//! Uses atomic operations for thread-safe concurrent updates.

use dashmap::DashMap;
use pingora_core::protocols::l4::socket::SocketAddr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::LazyLock;

/// Alpha parameter for EWMA calculation (0-100, representing 0.0-1.0)
/// Default: 20 (represents 0.2)
/// Higher values = more weight on recent samples, more responsive to changes
/// Lower values = more smoothing, less sensitive to spikes
static ALPHA_PERCENT: AtomicU32 = AtomicU32::new(20);

/// Initial EWMA value for new backends (1ms in microseconds)
/// New backends start with a low value to be prioritized for trial
const INITIAL_EWMA_US: u64 = 1_000;

/// Global EWMA values per backend address (in microseconds)
static EWMA_VALUES: LazyLock<DashMap<SocketAddr, AtomicU64>> = LazyLock::new(DashMap::new);

/// Update EWMA value for a backend address with new latency measurement.
///
/// EWMA formula: new_ewma = alpha * latency + (1 - alpha) * old_ewma
/// Using integer arithmetic: new_ewma = (alpha_percent * latency + (100 - alpha_percent) * old_ewma) / 100
///
/// # Arguments
/// * `addr` - Backend socket address
/// * `latency_us` - Current request latency in microseconds
#[inline]
pub fn update(addr: &SocketAddr, latency_us: u64) {
    let alpha = ALPHA_PERCENT.load(Ordering::Relaxed);

    // Performance optimization: avoid cloning by using entry_ref (requires dashmap 5.5+)
    // If not available, we need to clone once for entry API
    let entry = EWMA_VALUES
        .entry(addr.clone())
        .or_insert_with(|| AtomicU64::new(INITIAL_EWMA_US));

    // Use compare_exchange loop for better performance than fetch_update
    let mut current = entry.load(Ordering::Relaxed);
    loop {
        let new_ewma = (alpha as u64 * latency_us + (100 - alpha as u64) * current) / 100;
        match entry.compare_exchange_weak(current, new_ewma, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(actual) => current = actual,
        }
    }
}

/// Get current EWMA value for a backend address.
/// Returns the initial value if backend has no recorded metrics.
///
/// # Arguments
/// * `addr` - Backend socket address
///
/// # Returns
/// EWMA value in microseconds
#[inline]
pub fn get_ewma(addr: &SocketAddr) -> u64 {
    EWMA_VALUES
        .get(addr)
        .map(|v| v.load(Ordering::Relaxed))
        .unwrap_or(INITIAL_EWMA_US)
}

/// Remove EWMA entry for a backend address.
/// Used when a backend is removed from the pool.
///
/// # Arguments
/// * `addr` - Backend socket address
pub fn remove(addr: &SocketAddr) {
    EWMA_VALUES.remove(addr);
}

/// Set alpha parameter (smoothing factor) for EWMA calculation.
///
/// # Arguments
/// * `alpha` - Alpha value as percentage (0-100), where 20 = 0.2
///
/// # Examples
/// - Alpha = 10 (0.1): Slow response, more smoothing, stable environments
/// - Alpha = 20 (0.2): Default, balanced responsiveness and stability
/// - Alpha = 30 (0.3): Fast response, sensitive to changes, dynamic environments
pub fn set_alpha(alpha: u32) {
    let clamped = alpha.clamp(0, 100);
    ALPHA_PERCENT.store(clamped, Ordering::Relaxed);
    tracing::info!(alpha = clamped, "EWMA alpha parameter updated");
}

/// Get current alpha parameter
pub fn get_alpha() -> u32 {
    ALPHA_PERCENT.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr as StdSocketAddr;

    fn make_addr(port: u16) -> SocketAddr {
        let std_addr: StdSocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
        SocketAddr::Inet(std_addr)
    }

    #[test]
    fn test_initial_ewma() {
        let addr = make_addr(10001);
        // New backend should return initial value
        assert_eq!(get_ewma(&addr), INITIAL_EWMA_US);
    }

    #[test]
    fn test_ewma_update() {
        let addr = make_addr(10002);

        // First update sets baseline
        update(&addr, 5_000); // 5ms
        let ewma1 = get_ewma(&addr);

        // With alpha=0.2: new_ewma = 0.2 * 5000 + 0.8 * 1000 = 1800
        assert!(ewma1 > INITIAL_EWMA_US && ewma1 < 5_000);

        // Second update with higher latency
        update(&addr, 10_000); // 10ms
        let ewma2 = get_ewma(&addr);

        // EWMA should increase but not jump to 10ms
        assert!(ewma2 > ewma1 && ewma2 < 10_000);
    }

    #[test]
    fn test_ewma_smoothing() {
        let addr = make_addr(10003);

        // Simulate steady latency
        for _ in 0..10 {
            update(&addr, 2_000); // 2ms
        }

        let steady_ewma = get_ewma(&addr);

        // Should converge towards 2ms
        assert!(steady_ewma > 1_500 && steady_ewma < 2_500);
    }

    #[test]
    fn test_alpha_adjustment() {
        // Save original alpha
        let original = get_alpha();

        // Test setting alpha
        set_alpha(30);
        assert_eq!(get_alpha(), 30);

        set_alpha(10);
        assert_eq!(get_alpha(), 10);

        // Test clamping
        set_alpha(150);
        assert_eq!(get_alpha(), 100);

        // Restore original
        set_alpha(original);
    }

    #[test]
    fn test_remove() {
        let addr = make_addr(10004);

        update(&addr, 3_000);
        assert!(get_ewma(&addr) > INITIAL_EWMA_US);

        remove(&addr);
        // After removal, should return initial value again
        assert_eq!(get_ewma(&addr), INITIAL_EWMA_US);
    }

    #[test]
    fn test_concurrent_updates() {
        use std::thread;

        let addr = make_addr(10005);

        // Spawn multiple threads updating the same backend
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let addr = addr.clone();
                thread::spawn(move || {
                    for _ in 0..100 {
                        update(&addr, 1_000 + (i * 100));
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        // Should have some EWMA value without panicking
        let final_ewma = get_ewma(&addr);
        assert!(final_ewma > 0);
    }
}
