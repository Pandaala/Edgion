//! Webhook health check — active probing and passive monitoring.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use super::runtime::{now_epoch_secs, WebhookRuntime};

// ============================================================
// Health Check — Active Probe Task
// ============================================================

/// Spawn a background active health check task for a webhook service.
pub async fn spawn_active_health_check(webhook_ref: String, runtime: Arc<WebhookRuntime>) {
    let active = match runtime.config.health_check.as_ref().and_then(|hc| hc.active.as_ref()) {
        Some(a) => a.clone(),
        None => return,
    };
    let has_passive = runtime
        .config
        .health_check
        .as_ref()
        .and_then(|hc| hc.passive.as_ref())
        .is_some();

    let client = runtime.http_client.as_ref();
    let check_url = match &active.path {
        Some(path) => format!("{}{}", runtime.config.uri.trim_end_matches('/'), path),
        None => runtime.config.uri.clone(),
    };
    let interval = Duration::from_secs(active.interval_sec.max(1));
    let timeout = Duration::from_millis(active.timeout_ms.max(100));

    let mut consecutive_failures: u32 = 0;
    let mut consecutive_successes: u32 = 0;

    loop {
        tokio::time::sleep(interval).await;

        let result = client.get(&check_url).timeout(timeout).send().await;

        match result {
            Ok(resp) if resp.status().is_success() => {
                consecutive_failures = 0;
                consecutive_successes += 1;
                if consecutive_successes >= active.healthy_threshold {
                    if !runtime.healthy.load(Ordering::Relaxed) {
                        tracing::info!(webhook = %webhook_ref, "Webhook health restored by active probe");
                        runtime.passive_failures.store(0, Ordering::Relaxed);
                        runtime.backoff_sec.store(0, Ordering::Relaxed);
                    }
                    runtime.healthy.store(true, Ordering::Relaxed);
                }
            }
            _ => {
                consecutive_successes = 0;
                consecutive_failures += 1;
                // Only mark unhealthy from active probe if passive is NOT enabled
                if !has_passive && consecutive_failures >= active.unhealthy_threshold {
                    if runtime.healthy.load(Ordering::Relaxed) {
                        tracing::warn!(webhook = %webhook_ref, "Webhook marked unhealthy by active probe");
                    }
                    runtime.healthy.store(false, Ordering::Relaxed);
                }
            }
        }
    }
}

// ============================================================
// Health Check — Passive Monitoring
// ============================================================

/// Record a webhook call result for passive health monitoring.
pub fn record_passive_result(runtime: &WebhookRuntime, success: bool) {
    let passive = match runtime.config.health_check.as_ref().and_then(|hc| hc.passive.as_ref()) {
        Some(p) => p,
        None => return,
    };

    if success {
        runtime.passive_failures.store(0, Ordering::Relaxed);
        if !runtime.healthy.load(Ordering::Relaxed) {
            tracing::info!("Webhook health restored by successful request (half-open)");
            runtime.healthy.store(true, Ordering::Relaxed);
            runtime.backoff_sec.store(0, Ordering::Relaxed);
        }
    } else {
        let failures = runtime.passive_failures.fetch_add(1, Ordering::Relaxed) + 1;
        if failures >= passive.unhealthy_threshold && runtime.healthy.load(Ordering::Relaxed) {
            tracing::warn!(failures, "Webhook marked unhealthy by passive monitoring");
            runtime.healthy.store(false, Ordering::Relaxed);
            // Initialize backoff timer if passive-only
            let has_active = runtime
                .config
                .health_check
                .as_ref()
                .and_then(|hc| hc.active.as_ref())
                .is_some();
            if !has_active {
                if let Some(ref backoff) = passive.backoff {
                    runtime.backoff_sec.store(backoff.initial_sec, Ordering::Relaxed);
                    runtime.last_halfopen.store(now_epoch_secs(), Ordering::Relaxed);
                }
            }
        }
    }
}

/// Check if a half-open probe should be attempted (passive-only recovery).
pub fn should_halfopen_probe(runtime: &WebhookRuntime) -> bool {
    if runtime.healthy.load(Ordering::Relaxed) {
        return false;
    }
    let has_active = runtime
        .config
        .health_check
        .as_ref()
        .and_then(|hc| hc.active.as_ref())
        .is_some();
    if has_active {
        return false; // Active handles recovery
    }
    let passive = match runtime.config.health_check.as_ref().and_then(|hc| hc.passive.as_ref()) {
        Some(p) => p,
        None => return false,
    };
    let backoff = match &passive.backoff {
        Some(b) => b,
        None => return false,
    };

    let now = now_epoch_secs();
    let last = runtime.last_halfopen.load(Ordering::Relaxed);
    let interval = runtime.backoff_sec.load(Ordering::Relaxed);

    if interval == 0 || now >= last + interval {
        runtime.last_halfopen.store(now, Ordering::Relaxed);
        let next_backoff = ((interval as f64) * backoff.multiplier) as u64;
        runtime
            .backoff_sec
            .store(next_backoff.min(backoff.max_sec), Ordering::Relaxed);
        return true;
    }
    false
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};

    use crate::types::resources::link_sys::webhook::{PassiveHealthCheck, WebhookHealthCheck, WebhookServiceConfig};

    #[test]
    fn test_passive_health_check_marks_unhealthy() {
        let config = WebhookServiceConfig {
            uri: "http://localhost:8080".to_string(),
            health_check: Some(WebhookHealthCheck {
                active: None,
                passive: Some(PassiveHealthCheck {
                    unhealthy_threshold: 3,
                    failure_status_codes: vec![500, 502, 503, 504],
                    count_timeout: true,
                    backoff: None,
                }),
            }),
            ..Default::default()
        };

        let runtime = WebhookRuntime {
            config,
            http_client: std::sync::Arc::new(reqwest::Client::builder().build().expect("test client")),
            healthy: AtomicBool::new(true),
            passive_failures: AtomicU32::new(0),
            last_halfopen: AtomicU64::new(0),
            backoff_sec: AtomicU64::new(0),
            rate_counter: None,
        };

        // 3 consecutive failures should mark unhealthy
        record_passive_result(&runtime, false);
        assert!(runtime.healthy.load(Ordering::Relaxed)); // Still healthy after 1
        record_passive_result(&runtime, false);
        assert!(runtime.healthy.load(Ordering::Relaxed)); // Still healthy after 2
        record_passive_result(&runtime, false);
        assert!(!runtime.healthy.load(Ordering::Relaxed)); // Unhealthy after 3
    }

    #[test]
    fn test_passive_health_check_resets_on_success() {
        let config = WebhookServiceConfig {
            uri: "http://localhost:8080".to_string(),
            health_check: Some(WebhookHealthCheck {
                active: None,
                passive: Some(PassiveHealthCheck {
                    unhealthy_threshold: 3,
                    failure_status_codes: vec![500],
                    count_timeout: true,
                    backoff: None,
                }),
            }),
            ..Default::default()
        };

        let runtime = WebhookRuntime {
            config,
            http_client: std::sync::Arc::new(reqwest::Client::builder().build().expect("test client")),
            healthy: AtomicBool::new(true),
            passive_failures: AtomicU32::new(0),
            last_halfopen: AtomicU64::new(0),
            backoff_sec: AtomicU64::new(0),
            rate_counter: None,
        };

        record_passive_result(&runtime, false);
        record_passive_result(&runtime, false);
        // 2 failures, then success resets
        record_passive_result(&runtime, true);
        assert_eq!(runtime.passive_failures.load(Ordering::Relaxed), 0);
        assert!(runtime.healthy.load(Ordering::Relaxed));
    }
}
