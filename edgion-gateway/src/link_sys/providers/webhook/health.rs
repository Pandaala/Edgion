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
    let active = match runtime
        .config
        .conn
        .health_check
        .as_ref()
        .and_then(|hc| hc.active.as_ref())
    {
        Some(a) => a.clone(),
        None => return,
    };
    let has_passive = runtime
        .config
        .conn
        .health_check
        .as_ref()
        .and_then(|hc| hc.passive.as_ref())
        .is_some();

    let client = runtime.http_client.as_ref();
    let interval = Duration::from_secs(active.interval_sec.max(1));
    let timeout = Duration::from_millis(active.timeout_ms.max(100));

    let mut consecutive_failures: u32 = 0;
    let mut consecutive_successes: u32 = 0;

    loop {
        tokio::time::sleep(interval).await;

        // Re-evaluate on every tick so that a Service target with no ready
        // backend yet is skipped without restarting the health task.
        let Some(base) = runtime.resolved_target.pick_url() else {
            tracing::debug!(webhook = %webhook_ref, "Active health check skipped: no ready backend yet");
            continue;
        };
        let check_url = match &active.path {
            Some(path) => format!("{}{}", base.trim_end_matches('/'), path),
            None => base,
        };

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
    let passive = match runtime
        .config
        .conn
        .health_check
        .as_ref()
        .and_then(|hc| hc.passive.as_ref())
    {
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
                .conn
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
        .conn
        .health_check
        .as_ref()
        .and_then(|hc| hc.active.as_ref())
        .is_some();
    if has_active {
        return false; // Active handles recovery
    }
    let passive = match runtime
        .config
        .conn
        .health_check
        .as_ref()
        .and_then(|hc| hc.passive.as_ref())
    {
        Some(p) => p,
        None => return false,
    };
    let backoff = match &passive.backoff {
        Some(b) => b,
        None => return false,
    };

    let now = now_epoch_secs();
    let last = runtime.last_halfopen.load(Ordering::Acquire);
    let interval = runtime.backoff_sec.load(Ordering::Relaxed);

    if interval == 0 || now >= last + interval {
        // Only the CAS winner admits the probe. Concurrent callers that observed
        // the same `last` lose the swap and fall through to `false`, so exactly one
        // half-open probe is sent per backoff window instead of a thundering herd.
        // Mirrors SlidingWindowCounter::try_acquire.
        if runtime
            .last_halfopen
            .compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            let next_backoff = ((interval as f64) * backoff.multiplier) as u64;
            runtime
                .backoff_sec
                .store(next_backoff.min(backoff.max_sec), Ordering::Relaxed);
            return true;
        }
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

    use crate::link_sys::providers::webhook::compiled_request::CompiledOutboundRequest;
    use crate::link_sys::providers::webhook::runtime::WebhookResolvedTarget;
    use edgion_resources::resources::link_sys::webhook::{
        ExternalHttpConnection, PassiveBackoff, PassiveHealthCheck, WebhookHealthCheck, WebhookOutboundRequest,
        WebhookServiceConfig, WebhookTarget,
    };

    /// Compile a default (no-op) `CompiledOutboundRequest` for tests that only exercise
    /// health-state logic and don't care about request template behaviour.
    fn default_compiled_request() -> CompiledOutboundRequest {
        CompiledOutboundRequest::from_spec(&WebhookOutboundRequest::default())
            .expect("default request spec must compile")
    }

    #[test]
    fn test_passive_health_check_marks_unhealthy() {
        use edgion_resources::resources::http_route::BackendObjectReference;
        let config = WebhookServiceConfig {
            conn: ExternalHttpConnection {
                target: WebhookTarget {
                    backend_ref: BackendObjectReference {
                        group: String::new(),
                        kind: None,
                        name: "health-svc".to_string(),
                        namespace: Some("default".to_string()),
                        port: Some(8080),
                    },
                },
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
            },
            ..Default::default()
        };

        let runtime = WebhookRuntime {
            config,
            http_client: std::sync::Arc::new(reqwest::Client::builder().build().expect("test client")),
            resolved_target: WebhookResolvedTarget::Static("http://localhost:8080".to_string()),
            compiled_request: default_compiled_request(),
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
        use edgion_resources::resources::http_route::BackendObjectReference;
        let config = WebhookServiceConfig {
            conn: ExternalHttpConnection {
                target: WebhookTarget {
                    backend_ref: BackendObjectReference {
                        group: String::new(),
                        kind: None,
                        name: "health-svc".to_string(),
                        namespace: Some("default".to_string()),
                        port: Some(8080),
                    },
                },
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
            },
            ..Default::default()
        };

        let runtime = WebhookRuntime {
            config,
            http_client: std::sync::Arc::new(reqwest::Client::builder().build().expect("test client")),
            resolved_target: WebhookResolvedTarget::Static("http://localhost:8080".to_string()),
            compiled_request: default_compiled_request(),
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

    #[test]
    fn test_halfopen_probe_admits_exactly_one_under_contention() {
        use edgion_resources::resources::http_route::BackendObjectReference;
        use std::sync::{Arc, Barrier};

        let config = WebhookServiceConfig {
            conn: ExternalHttpConnection {
                target: WebhookTarget {
                    backend_ref: BackendObjectReference {
                        group: String::new(),
                        kind: None,
                        name: "health-svc".to_string(),
                        namespace: Some("default".to_string()),
                        port: Some(8080),
                    },
                },
                health_check: Some(WebhookHealthCheck {
                    active: None,
                    passive: Some(PassiveHealthCheck {
                        unhealthy_threshold: 3,
                        failure_status_codes: vec![500],
                        count_timeout: true,
                        backoff: Some(PassiveBackoff {
                            initial_sec: 5,
                            multiplier: 2.0,
                            max_sec: 60,
                        }),
                    }),
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        // Unhealthy with the backoff window already elapsed (last_halfopen = 0,
        // interval = 5, now = current epoch), so the gate is open for one probe.
        let runtime = Arc::new(WebhookRuntime {
            config,
            http_client: std::sync::Arc::new(reqwest::Client::builder().build().expect("test client")),
            resolved_target: WebhookResolvedTarget::Static("http://localhost:8080".to_string()),
            compiled_request: default_compiled_request(),
            healthy: AtomicBool::new(false),
            passive_failures: AtomicU32::new(0),
            last_halfopen: AtomicU64::new(0),
            backoff_sec: AtomicU64::new(5),
            rate_counter: None,
        });

        const THREADS: usize = 16;
        let barrier = Arc::new(Barrier::new(THREADS));
        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let rt = Arc::clone(&runtime);
                let b = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    b.wait();
                    should_halfopen_probe(&rt)
                })
            })
            .collect();

        let granted = handles
            .into_iter()
            .map(|h| h.join().expect("thread join"))
            .filter(|&won| won)
            .count();

        assert_eq!(
            granted, 1,
            "exactly one concurrent caller should win the half-open probe"
        );
    }
}
