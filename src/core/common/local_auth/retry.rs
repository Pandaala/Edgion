//! Background retry loop for installing the admin-credentials `LocalAuthState`
//! into a shared `UnifiedAuthState` when the initial K8s Secret fetch failed.
//!
//! On success the loop calls `update_local` and returns. On hard timeout it
//! calls the supplied `on_timeout` callback (production: `std::process::exit(1)`).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;
use tokio::time::Instant;

use crate::core::common::local_auth::{LocalAuthConfig, LocalAuthState};
use crate::core::common::unified_auth::UnifiedAuthState;

/// Abstraction over "fetch the Secret". Production wiring uses
/// `ensure_admin_credentials(conf_mgr)`; tests pass a stub.
pub type FetchFn = Box<
    dyn Fn() -> Pin<Box<dyn Future<Output = anyhow::Result<LocalAuthConfig>> + Send>>
        + Send
        + Sync,
>;

/// Abstraction over "exit the process on hard timeout". Production passes
/// `|| std::process::exit(1)`; tests pass a flag-setting closure.
pub type ExitFn = Box<dyn FnOnce() + Send + Sync>;

/// Run the retry loop: exponential backoff (1s → 2s → 4s → 8s → 16s → 30s cap),
/// hard deadline `timeout` wall-clock from call time.
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
    mut shutdown: broadcast::Receiver<()>,
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
            _ = shutdown.recv() => {
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
        let mut c = LocalAuthConfig::default();
        c.enabled = true;
        c.username = "admin".to_string();
        c.password = "a_long_enough_password_123".to_string();
        c.jwt_secret = "a_long_enough_jwt_secret_value_abcdef".to_string();
        c
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
        let (_tx, rx) = broadcast::channel(1);

        let handle = tokio::spawn(secret_retry_loop(
            state.clone(),
            fetch,
            Duration::from_secs(600),
            exit,
            rx,
        ));
        tokio::time::sleep(Duration::from_secs(8)).await;
        handle.await.unwrap();
        assert_eq!(attempt.load(Ordering::SeqCst), 3);
        assert!(state.local.load().is_some(), "state should be updated after successful fetch");
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
        let (_tx, rx) = broadcast::channel(1);

        let handle = tokio::spawn(secret_retry_loop(
            state.clone(),
            fetch,
            Duration::from_secs(600),
            exit,
            rx,
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
        let (tx, rx) = broadcast::channel(1);
        let handle = tokio::spawn(secret_retry_loop(state.clone(), fetch, Duration::from_secs(600), exit, rx));
        tokio::time::sleep(Duration::from_millis(100)).await;
        tx.send(()).unwrap();
        handle.await.unwrap();
    }
}
