//! Leader-gated conf_sync auto-CA rotation reconciler (spec §6 Task 5).
//!
//! A single background worker, started only on the elected leader (mirroring the
//! ACME service leadership wiring in `center.rs`), that on a 60s tick:
//! 1. GETs the gateway leaf Secret FRESH every tick (no caching — an out-of-band
//!    Secret loss must surface within one tick, not months later),
//! 2. asks the pure `cert_util::rotation_due` whether the client/server leaves
//!    are due (policy trigger on the client leaf window; emergency net on both),
//! 3. independently checks CA renewal on the ACTUAL adopted root window,
//! 4. rotates: Secret write FIRST (the more-widely-consumed artifact), then the
//!    in-process server identity store.
//!
//! Manual rotations (Admin API, Task 11) bypass the backoff and are enqueued on a
//! bounded channel; the tick path alone is backoff-gated.
//!
//! Leadership is expressed exactly at the `start_conf_sync_rotation` /
//! `stop_conf_sync_rotation` call sites (the single source of truth for the
//! `LEADER` flag), mirroring how ACME treats its start/stop.

use anyhow::Result;
use edgion_common::conf_sync::config::{ConfSyncRotationConfig, RotationPolicy};
use k8s_openapi::api::core::v1::Secret;
use kube::{Api, Client};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use time::OffsetDateTime;
use tokio::sync::mpsc;

use crate::conf_sync::conf_server::tls_identity::{global_handle, ServerIdentity};
use crate::services::conf_sync_ca::cert_util::{
    ca_renewal_due, last_firing, parse_cert_window, rotation_due, PolicyInputs, RotationDecision,
};
use crate::services::conf_sync_ca::{ensure_ca, read_leaf, write_ca, write_leaf};

// ============================================================================
// Public surface (Task 11 consumes these — exact names matter)
// ============================================================================

/// Which certificate material a rotation touches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotateScope {
    /// Rotate the client + server leaves against the current (adopted) root.
    Leaf,
    /// Renew the root (same key, new window) and then rotate both leaves.
    Ca,
}

/// Outcome of a manual rotation request (Admin API). Distinguishes a healthy
/// non-leader from a leader still mid-init so operators get an accurate reason
/// (spec §6 T5 — a bare "sender is None" would conflate the two).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerResult {
    /// Enqueued for the worker.
    Accepted,
    /// This replica is not the elected leader; rotation runs on the leader.
    NotLeader,
    /// Leader, but the rotation worker is not yet running (mid-init) or its queue
    /// is saturated (worker stuck).
    LeaderNotReady,
    /// auto_ca is off / `init()` was never called — no rotation service exists.
    NotConfigured,
}

// ============================================================================
// Global state
// ============================================================================

/// Immutable rotation parameters, set once from the cli auto_ca branch via
/// [`init`]. Absent in file mode / auto_ca off, which makes
/// [`start_conf_sync_rotation`] a no-op.
struct Params {
    ns: String,
    sans: Vec<String>,
    rotation: ConfSyncRotationConfig,
}

static PARAMS: OnceLock<Params> = OnceLock::new();

/// Handle to the running worker: the command sender plus the generation that
/// owns it (mirrors `ACME_GENERATION` — a stale handle from a prior leadership
/// term can never be confused with the current one).
struct Handle {
    tx: mpsc::Sender<RotateScope>,
    generation: u64,
}

static ROTATION_SERVICE: Mutex<Option<Handle>> = Mutex::new(None);

/// Monotonic generation counter. Each `start_conf_sync_rotation` allocates a
/// fresh generation. Starts at 0; the first running generation is 1.
static ROTATION_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Leadership flag — single source of truth, written ONLY at the start/stop call
/// sites (which are the leadership signal). Read by [`trigger_manual_rotation`]
/// and [`is_leader`].
static LEADER: AtomicBool = AtomicBool::new(false);

// ============================================================================
// init / lifecycle
// ============================================================================

/// Store the rotation parameters. Called once from the cli auto_ca branch. In
/// file mode / auto_ca off this is never called, so `start_conf_sync_rotation`
/// stays a no-op.
pub fn init(ns: String, sans: Vec<String>, rotation: ConfSyncRotationConfig) {
    if PARAMS.set(Params { ns, sans, rotation }).is_err() {
        tracing::warn!(
            component = "conf_sync_ca",
            "rotation::init called more than once; keeping the first parameters"
        );
    }
}

/// True when this replica currently holds leadership (per the start/stop sites).
pub fn is_leader() -> bool {
    LEADER.load(Ordering::SeqCst)
}

/// Generation of the currently running service, or 0 when none is running.
fn current_generation() -> u64 {
    ROTATION_SERVICE.lock().as_ref().map(|h| h.generation).unwrap_or(0)
}

/// Start the leader-gated rotation worker.
///
/// Sets the `LEADER` flag (this call site IS the leadership signal), then — only
/// when [`init`] has run (auto_ca) — spawns the single reconciler worker. Without
/// `init` it logs once at INFO and returns, so file-mode leaders do nothing here.
pub fn start_conf_sync_rotation(client: Client) {
    // Leadership is signalled by this call site regardless of auto_ca, so
    // `is_leader()` is accurate even in file mode.
    LEADER.store(true, Ordering::SeqCst);

    if PARAMS.get().is_none() {
        tracing::info!(
            component = "conf_sync_ca",
            "conf_sync rotation not initialized (auto_ca disabled or init not called); rotation service not started"
        );
        return;
    }

    let mut guard = ROTATION_SERVICE.lock();
    if guard.is_some() {
        tracing::warn!(
            component = "conf_sync_ca",
            "rotation service already running, call stop_conf_sync_rotation() first"
        );
        return;
    }
    let generation = ROTATION_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let (tx, rx) = mpsc::channel::<RotateScope>(16);
    *guard = Some(Handle { tx, generation });
    drop(guard); // release the lock before spawning

    tokio::spawn(async move {
        run(rx, client, generation).await;
    });

    tracing::info!(
        component = "conf_sync_ca",
        generation,
        "conf_sync rotation service started"
    );
}

/// Stop the rotation worker and clear the leadership flag.
///
/// Dropping the `Handle` drops the command `Sender`; the worker observes
/// `recv() => None` and exits its loop cleanly. Safe to call when nothing is
/// running (idempotent).
pub fn stop_conf_sync_rotation() {
    LEADER.store(false, Ordering::SeqCst);
    let mut guard = ROTATION_SERVICE.lock();
    if guard.take().is_some() {
        tracing::info!(component = "conf_sync_ca", "conf_sync rotation service stopped");
    }
}

// ============================================================================
// Manual trigger (Admin API — Task 11)
// ============================================================================

/// Pure decision for a manual rotation request. Factored out so the 2^3 truth
/// table is unit-testable without touching the globals.
fn decide_trigger(initialized: bool, leader: bool, running: bool) -> TriggerResult {
    if !initialized {
        TriggerResult::NotConfigured
    } else if !leader {
        TriggerResult::NotLeader
    } else if !running {
        TriggerResult::LeaderNotReady
    } else {
        TriggerResult::Accepted
    }
}

/// Request an out-of-band rotation of `scope`. Bypasses the tick backoff.
///
/// Resolves the reason via [`decide_trigger`] then enqueues on the bounded
/// worker channel. A FULL channel maps to `LeaderNotReady`: rotation is rare, so
/// a saturated 16-slot queue means the worker is stuck, not merely busy.
pub async fn trigger_manual_rotation(scope: RotateScope) -> TriggerResult {
    let initialized = PARAMS.get().is_some();
    let leader = LEADER.load(Ordering::SeqCst);
    // Clone the sender under the lock, then drop the guard before any send so no
    // sync lock is held across a suspension point.
    let sender = ROTATION_SERVICE.lock().as_ref().map(|h| h.tx.clone());
    let running = sender.is_some();

    match decide_trigger(initialized, leader, running) {
        TriggerResult::Accepted => {}
        other => return other,
    }

    let tx = sender.expect("decide_trigger==Accepted implies a running sender");
    // `try_send` (not `send().await`): a full queue must surface as
    // LeaderNotReady immediately rather than block the Admin API request.
    match tx.try_send(scope) {
        Ok(()) => TriggerResult::Accepted,
        Err(_) => TriggerResult::LeaderNotReady,
    }
}

// ============================================================================
// Retry backoff (tick path only)
// ============================================================================

/// Exponential backoff for the periodic check-and-rotate tick (spec §6 T2):
/// base 30s, doubling, capped at 600s. Pure — `ready()`/`bump()` take an
/// injected `Instant` so the progression is table-testable with no real sleep.
struct RetryBackoff {
    base: Duration,
    cap: Duration,
    /// The delay the NEXT `bump()` will impose. Starts at `base`, doubles (capped).
    next_delay: Duration,
    /// When the current backoff window elapses; `None` = ready now.
    ready_at: Option<Instant>,
}

impl RetryBackoff {
    fn new(base: Duration, cap: Duration) -> Self {
        Self {
            base,
            cap,
            next_delay: base,
            ready_at: None,
        }
    }

    /// Whether a tick may run now.
    fn ready(&self, now: Instant) -> bool {
        match self.ready_at {
            None => true,
            Some(t) => now >= t,
        }
    }

    /// Record a failure: open a backoff window of the current delay, then double
    /// the delay for next time (capped at `cap`).
    fn bump(&mut self, now: Instant) {
        self.ready_at = Some(now + self.next_delay);
        self.next_delay = (self.next_delay * 2).min(self.cap);
    }

    /// Record a success: back to immediate readiness and the base delay.
    fn reset(&mut self) {
        self.next_delay = self.base;
        self.ready_at = None;
    }
}

// ============================================================================
// Guard-skip dedup (spec §4.2)
// ============================================================================

/// Whether a scheduled guard-skip for `firing` should be logged. Returns true
/// only when `firing` differs from the last-logged firing, updating the tracker.
/// In-memory convenience only (not decision state).
fn should_log_skip(last_logged: &mut Option<OffsetDateTime>, firing: OffsetDateTime) -> bool {
    if *last_logged == Some(firing) {
        false
    } else {
        *last_logged = Some(firing);
        true
    }
}

// ============================================================================
// Worker
// ============================================================================

async fn run(mut rx: mpsc::Receiver<RotateScope>, client: Client, generation: u64) {
    // Defend against a start without init (should be impossible: start only
    // spawns when PARAMS is set). Never panic on the data plane.
    let Some(params) = PARAMS.get() else {
        tracing::error!(
            component = "conf_sync_ca",
            generation,
            "rotation worker started without init parameters; exiting"
        );
        return;
    };
    let secret_api: Api<Secret> = Api::namespaced(client, &params.ns);
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut backoff = RetryBackoff::new(Duration::from_secs(30), Duration::from_secs(600));
    // Guard-skip dedup tracker: in-memory convenience, reset per worker instance.
    let mut last_skip_firing: Option<OffsetDateTime> = None;

    tracing::info!(component = "conf_sync_ca", generation, "rotation worker started");

    loop {
        tokio::select! {
            cmd = rx.recv() => match cmd {
                Some(scope) => {
                    // Manual rotations bypass the backoff.
                    if let Err(e) = rotate(&secret_api, params, scope, "manual", generation).await {
                        tracing::warn!(
                            component = "conf_sync_ca",
                            error = %e,
                            ?scope,
                            "manual rotation failed"
                        );
                        edgion_common::observe::system_metrics::mark_system_error(
                            edgion_common::observe::system_metrics::components::CONF_SYNC_SERVER,
                            edgion_common::observe::system_metrics::reasons::CERT_ROTATION_FAILED,
                        );
                    }
                }
                None => break, // stop_conf_sync_rotation dropped the sender
            },
            _ = tick.tick() => {
                if !backoff.ready(Instant::now()) {
                    continue;
                }
                match check_and_rotate(&secret_api, params, &mut last_skip_firing, generation).await {
                    Ok(()) => {
                        backoff.reset();
                        edgion_common::observe::system_metrics::clear_system_error(
                            edgion_common::observe::system_metrics::components::CONF_SYNC_SERVER,
                            edgion_common::observe::system_metrics::reasons::CERT_ROTATION_FAILED,
                        );
                    }
                    Err(e) => {
                        backoff.bump(Instant::now());
                        tracing::warn!(
                            component = "conf_sync_ca",
                            error = %e,
                            "rotation check failed; will retry"
                        );
                        edgion_common::observe::system_metrics::mark_system_error(
                            edgion_common::observe::system_metrics::components::CONF_SYNC_SERVER,
                            edgion_common::observe::system_metrics::reasons::CERT_ROTATION_FAILED,
                        );
                    }
                }
            }
        }
    }

    tracing::info!(
        component = "conf_sync_ca",
        generation,
        "rotation worker stopped (channel closed)"
    );
}

/// Out-of-sync detection (spec: emergency re-key / out-of-band BYO-root swap):
/// either in-service cert not chaining to the CURRENT root means the trust
/// root changed under us — rotate immediately, regardless of policy.
pub(crate) fn out_of_sync_reason(client_leaf_pem: &str, server_cert_pem: &str, ca_pem: &str) -> Option<&'static str> {
    if !super::cert_util::chains_to(client_leaf_pem, ca_pem) {
        return Some("client leaf does not chain to current CA");
    }
    if !super::cert_util::chains_to(server_cert_pem, ca_pem) {
        return Some("server identity does not chain to current CA");
    }
    None
}

/// One periodic reconcile: fresh leaf GET, trust-root chain check, policy/emergency
/// decision, CA renewal.
async fn check_and_rotate(
    secret_api: &Api<Secret>,
    p: &Params,
    last_skip: &mut Option<OffsetDateTime>,
    generation: u64,
) -> Result<()> {
    let now = OffsetDateTime::now_utc();

    // Fresh GET every tick — NO caching (spec §6 T2). Missing/unparsable leaf =
    // immediately due.
    let (client_leaf_pem, client_leaf) = match read_leaf(secret_api).await? {
        Some((cert, _, _)) => match parse_cert_window(&cert) {
            Ok(w) => (cert, w),
            Err(_) => return rotate(secret_api, p, RotateScope::Leaf, "leaf secret unparsable", generation).await,
        },
        None => return rotate(secret_api, p, RotateScope::Leaf, "leaf secret missing", generation).await,
    };

    // Server leaf cert/window from the in-process identity. Scope the Arc so it
    // is not held across the later await.
    //
    // The handle can be legitimately unset for a short startup window (`init()`
    // runs synchronously before any spawn that could reach the leadership
    // callback, but the async `provision()` round-trip that populates the
    // handle is still in flight when the leader-gated worker's first tick
    // fires). Treat this as "not ready yet", not a failure: skip the tick at
    // DEBUG with no backoff bump and no system-error mark, since a transient
    // startup gap is not an operational error. This check stays ordered BEFORE
    // the chain checks below.
    let (server_cert_pem, server_leaf) = {
        let Some(handle) = global_handle().get() else {
            tracing::debug!(
                component = "conf_sync_ca",
                "server identity handle not initialized yet (provision still in flight); skipping rotation tick"
            );
            return Ok(());
        };
        let id = handle.load();
        (id.cert_pem.clone(), (id.not_before, id.not_after))
    };

    // `ensure_ca` runs ONCE after the handle-ready check: honors an out-of-band
    // BYO-root replacement, and its result feeds both the chain-consistency
    // check below and the CA-renewal due-check at the end. `rotate()` still
    // re-reads it internally for its own consistency window.
    let ca = ensure_ca(secret_api, &p.ns, p.rotation.ca_ttl.as_time()).await?;

    // Trust-root chain-consistency self-heal: leader failover after an emergency
    // re-key, or an out-of-band BYO-root swap with a different key, leaves an
    // in-service cert that no longer chains to the current root. Neither
    // scenario is caught by the validity-window due-checks below, so it is
    // checked FIRST and unconditionally rotates the leaves — regardless of
    // policy — skipping the rest of this tick.
    if let Some(reason) = out_of_sync_reason(&client_leaf_pem, &server_cert_pem, ca.ca_cert_pem()) {
        tracing::warn!(
            component = "conf_sync_ca",
            reason,
            event = "conf_sync_trust_root_resync",
            "in-service certificate no longer chains to the current CA; resyncing"
        );
        return rotate(secret_api, p, RotateScope::Leaf, reason, generation).await;
    }

    let last = match (&p.rotation.policy, &p.rotation.schedule) {
        (RotationPolicy::Scheduled, Some(s)) => last_firing(s, &p.rotation.schedule_timezone, now)?,
        _ => None,
    };

    let decision = rotation_due(
        &p.rotation,
        &PolicyInputs {
            client_leaf,
            server_leaf,
            last_firing: last,
        },
        now,
    );
    match decision {
        RotationDecision::Due(reason) => rotate(secret_api, p, RotateScope::Leaf, reason, generation).await?,
        RotationDecision::EmergencyDue(reason) => {
            tracing::warn!(
                component = "conf_sync_ca",
                reason,
                event = "cert_emergency_rotation",
                "emergency leaf rotation"
            );
            rotate(secret_api, p, RotateScope::Leaf, reason, generation).await?;
        }
        RotationDecision::NotDue => {
            // Log a scheduled guard-skip once per distinct firing (spec §4.2).
            if let Some(firing) = last {
                if should_log_skip(last_skip, firing) {
                    tracing::info!(
                        component = "conf_sync_ca",
                        firing = %firing,
                        "scheduled firing not due (guard not crossed); skipping"
                    );
                }
            }
        }
    }

    // CA renewal — independent due-check on the ACTUAL adopted root window
    // (spec §4.5), using the SAME `ca` fetched at the top of this tick.
    let ca_window = parse_cert_window(ca.ca_cert_pem())?;
    if ca_renewal_due(ca_window, now) {
        rotate(secret_api, p, RotateScope::Ca, "ca renewal threshold", generation).await?;
    }

    Ok(())
}

/// Perform a rotation. Re-reads `ensure_ca` on EVERY call (spec §6 T2: honors an
/// out-of-band BYO-root replacement). For `RotateScope::Ca`, renews the root
/// (same key, new window) and persists it FIRST, then rotates both leaves.
///
/// Order: gateway leaf Secret write FIRST (the more-widely-consumed artifact),
/// then the in-process server identity store.
async fn rotate(secret_api: &Api<Secret>, p: &Params, scope: RotateScope, reason: &str, generation: u64) -> Result<()> {
    // Generation guard (mirrors ACME_GENERATION): a rotate in-flight when this
    // replica lost leadership (channel dropped) then re-won it (new generation
    // spawned) must NOT clobber the new term's state. If we are no longer the
    // running generation, skip the rotation entirely.
    if current_generation() != generation {
        tracing::debug!(
            component = "conf_sync_ca",
            generation,
            "skipping rotation: stale generation (leadership flap)"
        );
        return Ok(());
    }

    let mut ca = ensure_ca(secret_api, &p.ns, p.rotation.ca_ttl.as_time()).await?;

    if matches!(scope, RotateScope::Ca) {
        ca = ca.renew_root_same_key(p.rotation.ca_ttl.as_time())?;
        // Persist renewed root (same key, new cert) — MUST preserve ca.key.
        write_ca(secret_api, &p.ns, &ca).await?;
        tracing::info!(component = "conf_sync_ca", "conf_sync_ca_renewed");
    }

    // Client leaf → Secret FIRST.
    let client_leaf = ca.sign_client(p.rotation.leaf_ttl.as_time())?;
    write_leaf(secret_api, &p.ns, &client_leaf, ca.ca_cert_pem()).await?;

    // Server leaf → in-process identity store.
    let server_leaf = ca.sign_server(&p.sans, p.rotation.leaf_ttl.as_time())?;
    let handle = global_handle()
        .get()
        .ok_or_else(|| anyhow::anyhow!("server identity handle not initialized"))?;
    let old_serial = handle.load().serial.clone();
    let new_identity =
        ServerIdentity::from_pems(&server_leaf.cert_pem, server_leaf.key_pem.as_str(), ca.ca_cert_pem())?;
    let (new_serial, new_na) = (new_identity.serial.clone(), new_identity.not_after);
    // Re-check generation right before the clobbering store: the awaits above
    // (Secret GET/apply) are the window in which leadership can flap. If we went
    // stale mid-flight, drop the freshly-minted identity rather than overwrite
    // the new term's served leaf.
    if current_generation() != generation {
        tracing::debug!(
            component = "conf_sync_ca",
            generation,
            "discarding rotated identity: generation changed mid-rotation"
        );
        return Ok(());
    }
    handle.store(new_identity);

    tracing::info!(
        component = "conf_sync_ca",
        reason,
        old_serial = %old_serial,
        new_serial = %new_serial,
        not_after = %new_na,
        "conf_sync_cert_rotated"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ================================================================
    // decide_trigger — full 2^3 truth table (8 cases → 4 outcomes)
    // ================================================================

    #[test]
    fn decide_trigger_truth_table() {
        struct Case {
            initialized: bool,
            leader: bool,
            running: bool,
            expected: TriggerResult,
        }
        let cases = [
            // Not initialized dominates regardless of leader/running.
            Case {
                initialized: false,
                leader: false,
                running: false,
                expected: TriggerResult::NotConfigured,
            },
            Case {
                initialized: false,
                leader: false,
                running: true,
                expected: TriggerResult::NotConfigured,
            },
            Case {
                initialized: false,
                leader: true,
                running: false,
                expected: TriggerResult::NotConfigured,
            },
            Case {
                initialized: false,
                leader: true,
                running: true,
                expected: TriggerResult::NotConfigured,
            },
            // Initialized but not leader → NotLeader (running irrelevant).
            Case {
                initialized: true,
                leader: false,
                running: false,
                expected: TriggerResult::NotLeader,
            },
            Case {
                initialized: true,
                leader: false,
                running: true,
                expected: TriggerResult::NotLeader,
            },
            // Leader but worker not running yet → LeaderNotReady.
            Case {
                initialized: true,
                leader: true,
                running: false,
                expected: TriggerResult::LeaderNotReady,
            },
            // Leader and running → Accepted.
            Case {
                initialized: true,
                leader: true,
                running: true,
                expected: TriggerResult::Accepted,
            },
        ];
        for c in &cases {
            assert_eq!(
                decide_trigger(c.initialized, c.leader, c.running),
                c.expected,
                "initialized={} leader={} running={}",
                c.initialized,
                c.leader,
                c.running
            );
        }
    }

    // ================================================================
    // RetryBackoff — base 30s, x2, 600s cap; ready()/bump()/reset()
    // ================================================================

    #[test]
    fn retry_backoff_progression_and_readiness() {
        let base = Duration::from_secs(30);
        let cap = Duration::from_secs(600);
        let t0 = Instant::now();
        let mut b = RetryBackoff::new(base, cap);

        // Fresh backoff is immediately ready.
        assert!(b.ready(t0), "fresh backoff must be ready");

        // First failure: 30s window, then not-ready until it elapses.
        b.bump(t0);
        assert!(!b.ready(t0 + Duration::from_secs(29)), "must not be ready at 29s");
        assert!(b.ready(t0 + Duration::from_secs(30)), "must be ready at 30s");

        // Doubling sequence of imposed windows: 30 → 60 → 120 → 240 → 480 → 600 (cap).
        let mut b = RetryBackoff::new(base, cap);
        let expected = [30u64, 60, 120, 240, 480, 600, 600, 600];
        let mut cursor = t0;
        for (i, want) in expected.iter().enumerate() {
            b.bump(cursor);
            let w = Duration::from_secs(*want);
            assert!(
                !b.ready(cursor + w - Duration::from_secs(1)),
                "bump #{i}: must not be ready just before {want}s"
            );
            assert!(b.ready(cursor + w), "bump #{i}: must be ready at {want}s");
            // Advance the clock past this window for the next bump.
            cursor += w;
        }

        // reset() restores immediate readiness AND the base delay.
        b.reset();
        assert!(b.ready(cursor), "reset must restore immediate readiness");
        b.bump(cursor);
        assert!(
            !b.ready(cursor + Duration::from_secs(29)),
            "post-reset window is base (30s)"
        );
        assert!(
            b.ready(cursor + Duration::from_secs(30)),
            "post-reset window is base (30s)"
        );
    }

    // ================================================================
    // should_log_skip — once per distinct firing
    // ================================================================

    #[test]
    fn should_log_skip_dedups_per_firing() {
        use time::macros::datetime;
        let a = datetime!(2026-01-04 03:00 UTC);
        let b = datetime!(2026-01-11 03:00 UTC);
        let mut last: Option<OffsetDateTime> = None;

        // First sight of A → log.
        assert!(should_log_skip(&mut last, a));
        // Repeat A → suppressed.
        assert!(!should_log_skip(&mut last, a));
        assert!(!should_log_skip(&mut last, a));
        // New firing B → log.
        assert!(should_log_skip(&mut last, b));
        assert!(!should_log_skip(&mut last, b));
        // Back to A (last was B) → log again.
        assert!(should_log_skip(&mut last, a));
    }

    // ================================================================
    // out_of_sync_reason — trust-root chain-consistency (emergency re-key /
    // out-of-band BYO-root swap self-heal)
    // ================================================================

    use crate::services::conf_sync_ca::Ca;

    #[test]
    fn out_of_sync_reason_table() {
        let ca_a = Ca::generate_root(time::Duration::days(3650)).unwrap();
        let ca_b = Ca::generate_root(time::Duration::days(3650)).unwrap();
        let client_a = ca_a.sign_client(time::Duration::days(30)).unwrap();
        let server_a = ca_a
            .sign_server(&["example.svc".into()], time::Duration::days(30))
            .unwrap();
        let client_b = ca_b.sign_client(time::Duration::days(30)).unwrap();
        let server_b = ca_b
            .sign_server(&["example.svc".into()], time::Duration::days(30))
            .unwrap();

        // In-sync: both leaves chain to the current CA -> None.
        assert!(out_of_sync_reason(&client_a.cert_pem, &server_a.cert_pem, ca_a.ca_cert_pem()).is_none());

        // Stale client leaf only -> Some(client...).
        let reason = out_of_sync_reason(&client_b.cert_pem, &server_a.cert_pem, ca_a.ca_cert_pem());
        assert_eq!(reason, Some("client leaf does not chain to current CA"));

        // Stale server leaf only -> Some(server...).
        let reason = out_of_sync_reason(&client_a.cert_pem, &server_b.cert_pem, ca_a.ca_cert_pem());
        assert_eq!(reason, Some("server identity does not chain to current CA"));

        // Both stale -> client reason wins (checked first).
        let reason = out_of_sync_reason(&client_b.cert_pem, &server_b.cert_pem, ca_a.ca_cert_pem());
        assert_eq!(reason, Some("client leaf does not chain to current CA"));

        // Renewed-same-key root: leaves signed by the OLD root still chain to the
        // renewed root's cert per issuer/key equivalence... actually the check is
        // against the CURRENT (renewed) CA cert with leaves signed by the renewed
        // CA -> None (normal renewal never trips the check).
        let renewed = ca_a.renew_root_same_key(time::Duration::days(3650)).unwrap();
        let client_renewed = renewed.sign_client(time::Duration::days(30)).unwrap();
        let server_renewed = renewed
            .sign_server(&["example.svc".into()], time::Duration::days(30))
            .unwrap();
        assert!(out_of_sync_reason(
            &client_renewed.cert_pem,
            &server_renewed.cert_pem,
            renewed.ca_cert_pem()
        )
        .is_none());
        // And leaves signed by the OLD root still chain to the OLD root cert (sanity,
        // same-key property, mirrors cert_util's dedicated test).
        assert!(out_of_sync_reason(&client_a.cert_pem, &server_a.cert_pem, ca_a.ca_cert_pem()).is_none());
    }
}
