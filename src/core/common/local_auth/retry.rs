//! Background retry loop for installing the admin-credentials `LocalAuthState`
//! into a shared `UnifiedAuthState` when the initial K8s Secret fetch failed.
//!
//! On success the loop calls `update_local` and returns. On hard timeout it
//! calls the supplied `on_timeout` callback (production: `std::process::exit(1)`).
//!
//! Shutdown signaling uses `tokio_util::sync::CancellationToken` rather than
//! `broadcast::Receiver`: its semantics resolve only on explicit `cancel()`, never
//! on drop, so the loop cannot exit prematurely when the caller drops the `Sender`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::core::common::local_auth::{LocalAuthConfig, LocalAuthState};
use crate::core::common::unified_auth::UnifiedAuthState;

/// Abstraction over "fetch the Secret". Production wiring uses
/// `ensure_admin_credentials(conf_mgr)`; tests pass a stub.
pub type FetchFn = Box<dyn Fn() -> Pin<Box<dyn Future<Output = anyhow::Result<LocalAuthConfig>> + Send>> + Send + Sync>;

/// Abstraction over "exit the process on hard timeout". Production passes
/// `|| std::process::exit(1)`; tests pass a flag-setting closure.
pub type ExitFn = Box<dyn FnOnce() + Send + Sync>;

/// Run the retry loop: exponential backoff (1s → 2s → 4s → 8s → 16s → 30s cap),
/// approximate deadline `timeout` wall-clock from call time.
///
/// The deadline is checked after each sleep, before the next fetch. If a
/// fetch is already in-flight when the deadline passes, the loop lets it
/// complete before enforcing the deadline — effective exit time is
/// `deadline + fetch_latency`, typically at most a few seconds.
///
/// - On fetch success: builds `LocalAuthState`, calls `auth_state.update_local`,
///   logs `info!`, returns.
/// - On fetch error with time remaining: logs `warn!`, sleeps, continues.
/// - On deadline: logs `error!`, calls `on_timeout`, returns.
/// - On shutdown signal: returns cleanly without touching state.
pub async fn secret_retry_loop(
    auth_state: Arc<UnifiedAuthState>,
    fetch: FetchFn,
    timeout: Duration,
    on_timeout: ExitFn,
    shutdown: CancellationToken,
) {
    let component = "secret_retry";
    let deadline = Instant::now() + timeout;
    let mut delay = Duration::from_secs(1);
    const CAP: Duration = Duration::from_secs(30);

    tracing::info!(
        component = component,
        timeout_secs = timeout.as_secs(),
        "Admin credentials Secret retry loop started"
    );

    loop {
        tokio::select! {
            // CancellationToken::cancelled() 的语义恰好匹配 retry loop 的意图：
            // - token.cancel() 调用后 resolve → 正常关停
            // - token 被 drop 但未 cancel → 永不 resolve（不会误触关停）
            _ = shutdown.cancelled() => {
                tracing::info!(component = component, "retry loop received shutdown, exiting");
                return;
            }
            _ = tokio::time::sleep(delay) => {}
        }

        if Instant::now() >= deadline {
            tracing::error!(
                component = component,
                "Admin credentials Secret fetch timed out after {}s, triggering process exit",
                timeout.as_secs()
            );
            on_timeout();
            return;
        }

        match fetch().await {
            Ok(cfg) => {
                let new_state = LocalAuthState::from_config(&cfg);
                auth_state.update_local(new_state);
                tracing::info!(
                    component = component,
                    "Admin credentials Secret loaded after retry; auth state now Ready"
                );
                return;
            }
            Err(e) => {
                tracing::warn!(
                    component = component,
                    error = %e,
                    delay_secs = delay.as_secs(),
                    "Secret fetch failed; retrying after backoff"
                );
                delay = (delay * 2).min(CAP);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn valid_local() -> LocalAuthConfig {
        LocalAuthConfig {
            username: "admin".to_string(),
            password: "a_long_enough_password_123".to_string(),
            jwt_secret: "a_long_enough_jwt_secret_value_abcdef".to_string(),
            ..LocalAuthConfig::default()
        }
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn retry_loop_success_on_third_attempt_updates_state() {
        let state = UnifiedAuthState::from_configs(None, None, true, "test").unwrap();
        let attempt = Arc::new(AtomicU32::new(0));
        let attempt_clone = attempt.clone();
        let fetch: FetchFn = Box::new(move || {
            let a = attempt_clone.clone();
            Box::pin(async move {
                let n = a.fetch_add(1, Ordering::SeqCst) + 1;
                if n < 3 {
                    Err(anyhow::anyhow!("transient error #{}", n))
                } else {
                    Ok(valid_local())
                }
            })
        });
        let timed_out = Arc::new(AtomicU32::new(0));
        let t = timed_out.clone();
        let exit: ExitFn = Box::new(move || {
            t.fetch_add(1, Ordering::SeqCst);
        });
        let shutdown = CancellationToken::new();

        let handle = tokio::spawn(secret_retry_loop(
            state.clone(),
            fetch,
            Duration::from_secs(600),
            exit,
            shutdown.clone(),
        ));
        tokio::time::sleep(Duration::from_secs(8)).await;
        handle.await.unwrap();
        assert_eq!(attempt.load(Ordering::SeqCst), 3);
        assert!(
            state.local.load().is_some(),
            "state should be updated after successful fetch"
        );
        assert_eq!(timed_out.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn retry_loop_times_out_and_calls_on_timeout() {
        let state = UnifiedAuthState::from_configs(None, None, true, "test").unwrap();
        let attempt = Arc::new(AtomicU32::new(0));
        let attempt_clone = attempt.clone();
        let fetch: FetchFn = Box::new(move || {
            let a = attempt_clone.clone();
            Box::pin(async move {
                a.fetch_add(1, Ordering::SeqCst);
                Err(anyhow::anyhow!("persistent error"))
            })
        });
        let timed_out = Arc::new(AtomicU32::new(0));
        let t = timed_out.clone();
        let exit: ExitFn = Box::new(move || {
            t.fetch_add(1, Ordering::SeqCst);
        });
        let shutdown = CancellationToken::new();

        let handle = tokio::spawn(secret_retry_loop(
            state.clone(),
            fetch,
            Duration::from_secs(600),
            exit,
            shutdown.clone(),
        ));
        tokio::time::sleep(Duration::from_secs(601)).await;
        handle.await.unwrap();
        assert!(attempt.load(Ordering::SeqCst) > 0);
        assert_eq!(timed_out.load(Ordering::SeqCst), 1);
        assert!(state.local.load().is_none());
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn retry_loop_stops_on_shutdown() {
        let state = UnifiedAuthState::from_configs(None, None, true, "test").unwrap();
        let fetch: FetchFn = Box::new(|| Box::pin(async move { Err(anyhow::anyhow!("never used")) }));
        let exit: ExitFn = Box::new(|| panic!("on_timeout should not fire"));
        let shutdown = CancellationToken::new();
        let handle = tokio::spawn(secret_retry_loop(
            state.clone(),
            fetch,
            Duration::from_secs(600),
            exit,
            shutdown.clone(),
        ));
        tokio::time::sleep(Duration::from_millis(100)).await;
        shutdown.cancel();
        handle.await.unwrap();
    }

    /// Regression guard: dropping the shutdown `Sender` immediately after spawning
    /// must not cause the retry loop to exit on its first iteration.
    /// `broadcast::Receiver::recv()` returns `Err(Closed)` when the sender is dropped,
    /// and a naïve `_ = shutdown.recv() =>` arm would match that error and exit early —
    /// the loop must tolerate a dropped sender and continue retrying until success or deadline.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn retry_loop_keeps_running_when_shutdown_tx_dropped() {
        let state = UnifiedAuthState::from_configs(None, None, true, "test").unwrap();
        let attempt = Arc::new(AtomicU32::new(0));
        let attempt_clone = attempt.clone();
        let fetch: FetchFn = Box::new(move || {
            let a = attempt_clone.clone();
            Box::pin(async move {
                let n = a.fetch_add(1, Ordering::SeqCst) + 1;
                if n < 2 {
                    Err(anyhow::anyhow!("transient"))
                } else {
                    Ok(valid_local())
                }
            })
        });
        let exit: ExitFn = Box::new(|| panic!("on_timeout should not fire"));
        // Production pattern: outer drops its token handle while only a clone remains
        // in the retry task. Simulates "Sender dropped but cancel() never called";
        // under CancellationToken semantics, shutdown.cancelled() should NOT resolve,
        // so the retry loop must keep running until fetch succeeds or deadline hits.
        let outer_token = CancellationToken::new();
        let retry_shutdown = outer_token.clone();
        drop(outer_token);

        let handle = tokio::spawn(secret_retry_loop(
            state.clone(),
            fetch,
            Duration::from_secs(600),
            exit,
            retry_shutdown,
        ));
        tokio::time::sleep(Duration::from_secs(4)).await;
        handle.await.unwrap();
        assert!(
            attempt.load(Ordering::SeqCst) >= 2,
            "retry loop exited without attempting fetches; attempt = {}",
            attempt.load(Ordering::SeqCst)
        );
        assert!(
            state.local.load().is_some(),
            "state should have been populated on the 2nd attempt"
        );
    }
}
