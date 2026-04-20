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
pub type FetchFn = Box<dyn Fn() -> Pin<Box<dyn Future<Output = anyhow::Result<LocalAuthConfig>> + Send>> + Send + Sync>;

/// Abstraction over "exit the process on hard timeout". Production passes
/// `|| std::process::exit(1)`; tests pass a flag-setting closure.
pub type ExitFn = Box<dyn FnOnce() + Send + Sync>;

/// Returns a future that resolves only on an explicit `Ok(())` broadcast.
///
/// `Err(Closed)` is returned synchronously and repeatedly once all senders
/// drop — we don't want that to race against the sleep arm of the retry
/// loop's `select!`, so in that case we block forever. `Err(Lagged)` just
/// means we missed some signals; try again.
async fn wait_for_shutdown(shutdown: &mut broadcast::Receiver<()>) {
    loop {
        match shutdown.recv().await {
            Ok(()) => return,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => {
                std::future::pending::<()>().await;
                unreachable!();
            }
        }
    }
}

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
            // Only a real `Ok(())` broadcast counts as a shutdown signal.
            // `broadcast::Receiver::recv()` returns `Err(Closed)` immediately
            // and repeatedly once all `Sender`s are dropped, which production
            // callers do since the retry task is time-bounded. Without the
            // pending-on-closed wrapper below, `select!` would match the
            // Err every iteration and the task would exit on its first poll
            // without attempting a single fetch.
            _ = wait_for_shutdown(&mut shutdown) => {
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
        let handle = tokio::spawn(secret_retry_loop(
            state.clone(),
            fetch,
            Duration::from_secs(600),
            exit,
            rx,
        ));
        tokio::time::sleep(Duration::from_millis(100)).await;
        tx.send(()).unwrap();
        handle.await.unwrap();
    }

    /// Regression guard: if the caller drops the broadcast `Sender` immediately
    /// after spawning (which `serve`/`serve_with_shutdown` did in the initial
    /// fix #01 wiring), `broadcast::Receiver::recv()` returns `Err(Closed)` on
    /// the first poll. `tokio::select!`'s `_ = shutdown.recv() =>` arm pattern
    /// matches any resolved value — including `Err` — so the task would exit
    /// on its first iteration, never attempting a single fetch.
    ///
    /// After the fix, the loop must tolerate a dropped `Sender` and continue
    /// retrying until success or deadline.
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
        // Production pattern: tx is dropped immediately; only rx is kept.
        let (tx, rx) = broadcast::channel::<()>(1);
        drop(tx);

        let handle = tokio::spawn(secret_retry_loop(
            state.clone(),
            fetch,
            Duration::from_secs(600),
            exit,
            rx,
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
