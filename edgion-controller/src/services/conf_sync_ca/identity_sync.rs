//! Per-replica conf_sync server-identity reconciler — runs on EVERY controller
//! replica, leader or not.
//!
//! # Why this exists
//!
//! `ha_mode: all-serve` is a supported controller mode in which **every** replica
//! serves the conf_sync gRPC API (`set_config_sync_server(Some(..))` is called
//! unconditionally once caches are ready; only `start_leader_services` is
//! leader-gated). That API streams **resolved Secrets** — TLS private keys,
//! backend credentials — to the data plane.
//!
//! But the serving TLS identity (`ServerIdentityHandle`, which carries BOTH the
//! served leaf AND the client-CA verifier used to authenticate gateways) was
//! built exactly once, at process start, from `provision()`. The only thing that
//! ever called `handle.store()` afterwards was the LEADER's rotation reconciler
//! (`rotation.rs`) plus the file-mode cert watcher (which is not spawned under
//! `auto_ca`).
//!
//! Consequence, before this module existed: under `auto_ca` + `all-serve`, a
//! non-leader replica served resolved Secrets behind a trust root frozen at its
//! own boot — forever. After an emergency CA re-key
//! (`cicd/deploy/kubernetes/scripts/rekey_conf_sync_ca.sh`), the compromised
//! gateway client leaf still authenticated against every non-leader and still
//! received the full Secret stream, until that pod happened to restart. The
//! feature's only revocation primitive did not revoke.
//!
//! # The istiod precedent
//!
//! Istio solves exactly this with `watchRootCertAndGenKeyCert`
//! (`pilot/pkg/bootstrap/certcontroller.go`): it runs unconditionally on every
//! istiod replica, polls the root on an interval, and when the root changes it
//! re-signs *that pod's own* serving cert. Istio's leader election covers only
//! controllers that must not run concurrently (status, namespace, gateway,
//! analysis) — "the critical path (xDS, injection, certificates) does not depend
//! on leader election". The CA signing key lives in a Kubernetes Secret precisely
//! so that every replica can read it and sign for itself.
//!
//! # The split, as implemented
//!
//! | who | does what | why |
//! |---|---|---|
//! | **leader only** (`rotation.rs`) | WRITES Secrets: rotates the CA and the shared gateway client leaf | single-writer; must be serialized |
//! | **every replica** (this module) | keeps **its own** server identity in sync with the current root | read-only on Secrets, writes only its own in-memory handle → needs zero coordination |
//!
//! This loop therefore NEVER calls `ensure_ca` (a write path that POST-creates a
//! root on absence, which from a non-leader would split-brain the trust anchor).
//! It calls the read-only `load_ca`, and treats absence as "the leader has not
//! bootstrapped yet".

use edgion_common::conf_sync::config::ConfSyncRotationConfig;
use edgion_common::observe::system_metrics::{
    clear_system_error, components::CONF_SYNC_SERVER, mark_system_error, reasons::CERT_IDENTITY_SYNC_FAILED,
};
use k8s_openapi::api::core::v1::Secret;
use kube::{Api, Client};
use std::time::Duration;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;

use crate::conf_sync::conf_server::tls_identity::{global_handle, ServerIdentity, ServerIdentityHandle};
use crate::services::conf_sync_ca::cert_util::chains_to;
use crate::services::conf_sync_ca::load_ca;

/// Poll interval for the per-replica identity reconciler. Matches istiod's
/// `rootCertPollingInterval` (60s).
const IDENTITY_SYNC_INTERVAL: Duration = Duration::from_secs(60);

/// Should THIS replica re-sign its own serving leaf?
///
/// `None` = no. Reasons are `&'static str` so they can be logged directly.
///
/// Order:
/// 1. the leaf does not chain to the CURRENT root → re-sign (this is the
///    emergency-re-key / BYO-root-swap case the whole module exists for),
/// 2. the identity's trust anchor is not the current root CERTIFICATE → re-sign.
///    This arm exists because a **same-key root renewal** mints a different root
///    cert that every old leaf still chains to: arm 1 sees `chains == true` and
///    would leave this replica's client-CA verifier holding the OLD root cert for
///    up to half a leaf lifetime. That is benign only because webpki keys trust
///    anchors on subject+SPKI (both unchanged by a same-key renewal) — an
///    implementation detail we refuse to depend on. Mirrors `rotation.rs`'s
///    `out_of_sync_reason` arm 3 ("leaf secret ca.crt is not the current root
///    cert"), which was added for exactly this blind spot on the client-leaf side.
/// 3. the leaf is inside the emergency window → re-sign,
/// 4. the leaf is past `lifetime_ratio` of its own validity → re-sign,
/// 5. otherwise → no.
///
/// **Deliberate design point:** this ALWAYS uses the simple half-life
/// (`lifetime_ratio`) rule and deliberately IGNORES the `scheduled` / `manual`
/// rotation policies. Those policies exist to control when the SHARED GATEWAY
/// CLIENT LEAF SECRET changes — a change that disrupts every gateway in the
/// fleet and therefore belongs in a maintenance window. Re-signing this
/// replica's own in-memory server leaf disrupts nothing: established TLS
/// sessions are unaffected (certificates are consumed only at handshake) and new
/// handshakes simply pick up the new cert. There is no reason to defer it, and
/// deferring it under a `manual` policy would recreate the very bug this module
/// fixes.
pub(crate) fn identity_resign_reason(
    chains: bool,
    identity_ca_pem: &str,
    current_ca_pem: &str,
    window: (OffsetDateTime, OffsetDateTime),
    lifetime_ratio: f64,
    emergency: time::Duration,
    now: OffsetDateTime,
) -> Option<&'static str> {
    if !chains {
        return Some("server identity does not chain to the current root");
    }
    // Byte compare, whitespace-trimmed (same normalization as `out_of_sync_reason`):
    // the anchor this identity's verifier was built from must BE the current root
    // cert, not merely a root the leaf happens to chain to.
    if identity_ca_pem.trim() != current_ca_pem.trim() {
        return Some("server identity trust anchor is not the current root cert");
    }
    let (not_before, not_after) = window;
    if not_after - now <= emergency {
        return Some("server leaf within the emergency window");
    }
    if now >= not_before + (not_after - not_before) * lifetime_ratio {
        return Some("server leaf past its lifetime ratio");
    }
    None
}

/// Spawn the per-replica server-identity reconciler.
///
/// **NOT leader-gated, and deliberately without a generation guard or a
/// start/stop pairing**: it is a per-PROCESS task that lives for the lifetime of
/// the process. It is the exact opposite of the ACME-style leader services —
/// there is no leadership term to scope it to, nothing it writes can conflict
/// with another replica (its only mutation is this process's own in-memory
/// `ServerIdentityHandle`), and it must keep running precisely on the replicas
/// that are NOT the leader.
///
/// Call once, from the `auto_ca` startup branch, after `global_handle().set(..)`.
///
/// `shutdown` is the same token every other conf_sync background task is wired to: on
/// graceful shutdown this loop stops issuing Secret GETs instead of ticking on into a
/// half-torn-down process.
pub fn start_identity_sync(
    client: Client,
    ns: String,
    sans: Vec<String>,
    rotation: ConfSyncRotationConfig,
    shutdown: CancellationToken,
) {
    tokio::spawn(async move {
        let secret_api: Api<Secret> = Api::namespaced(client, &ns);
        let mut tick = tokio::time::interval(IDENTITY_SYNC_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        tracing::info!(
            component = "conf_sync_ca",
            interval_secs = IDENTITY_SYNC_INTERVAL.as_secs(),
            "per-replica conf_sync server identity reconciler started"
        );
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    tracing::info!(
                        component = "conf_sync_ca",
                        "per-replica conf_sync server identity reconciler shutting down"
                    );
                    break;
                }
                _ = tick.tick() => {}
            }
            // A single failed tick must never kill the task: log, mark, and retry
            // on the next tick (the 60s cadence IS the backoff — this loop does no
            // writes, so a hot retry buys nothing).
            //
            // The mark is CERT_IDENTITY_SYNC_FAILED, NOT the rotation reconciler's
            // CERT_ROTATION_FAILED. **Never let a loop clear a mark it did not set.**
            // Both loops run on the leader under the same `conf_sync_server` component,
            // but this one is read-only and therefore succeeds almost always (a tick
            // with nothing to do — or with the CA Secret absent — is an `Ok(())`). If it
            // shared the rotation reason, it would drive that gauge to 0 every 60s while
            // the rotation reconciler (backing off up to 600s between real attempts) was
            // marking it 1 — and the documented `cert_rotation_failed == 1 for 5m` PAGE
            // would essentially never fire. The realistic sustained rotation failure is a
            // WRITE failure (RBAC denied on the Secret apply, signing error), which this
            // read-only loop does not share and would happily clear. The reverse also
            // matters: a failing tick here on a NON-leader must not report "the rotation
            // reconciler is failing" about a loop that does not even run on that pod.
            match sync_once(&secret_api, &sans, &rotation).await {
                Ok(()) => clear_system_error(CONF_SYNC_SERVER, CERT_IDENTITY_SYNC_FAILED),
                Err(e) => {
                    // `{:#}` renders the full anyhow cause chain (matching rotation.rs);
                    // plain Display would hide the real failure.
                    tracing::warn!(
                        component = "conf_sync_ca",
                        error = %format_args!("{e:#}"),
                        "per-replica server identity sync failed; will retry on the next tick"
                    );
                    mark_system_error(CONF_SYNC_SERVER, CERT_IDENTITY_SYNC_FAILED);
                }
            }
        }
    });
}

/// One reconcile pass. Read-only on Kubernetes; the only mutation is this
/// process's own `ServerIdentityHandle`.
async fn sync_once(secret_api: &Api<Secret>, sans: &[String], rotation: &ConfSyncRotationConfig) -> anyhow::Result<()> {
    // No conf_sync TLS at all (plaintext / skip_tls) — nothing to keep in sync.
    let Some(handle) = global_handle().get() else {
        return Ok(());
    };

    sync_once_with_handle(secret_api, sans, rotation, handle).await
}

async fn sync_once_with_handle(
    secret_api: &Api<Secret>,
    sans: &[String],
    rotation: &ConfSyncRotationConfig,
    handle: &ServerIdentityHandle,
) -> anyhow::Result<()> {
    // Capture before acquisition so a writer completing during the CA GET cannot
    // become the expected identity for a decision based on the older fetched CA.
    let expected = handle.load();

    // READ-ONLY (never `ensure_ca`): absence means the leader has not bootstrapped
    // the CA Secret yet. That is normal at cold start, so it is a DEBUG skip, not
    // an error — and definitely not a reason to mint a root from a non-leader.
    let Some(ca) = load_ca(secret_api).await? else {
        tracing::debug!(
            component = "conf_sync_ca",
            "CA secret not present yet (leader has not bootstrapped it); skipping identity sync tick"
        );
        return Ok(());
    };

    let old_serial = &expected.serial;
    let chains = chains_to(&expected.cert_pem, ca.ca_cert_pem());

    let Some(reason) = identity_resign_reason(
        chains,
        &expected.ca_cert_pem,
        ca.ca_cert_pem(),
        (expected.not_before, expected.not_after),
        rotation.lifetime_ratio,
        rotation.emergency_rotate_below.as_time(),
        OffsetDateTime::now_utc(),
    ) else {
        return Ok(());
    };

    let leaf = ca.sign_server(sans, rotation.leaf_ttl.as_time())?;
    let new_identity = ServerIdentity::from_pems(&leaf.cert_pem, leaf.key_pem.as_str(), ca.ca_cert_pem())?;
    let (new_serial, not_after) = (new_identity.serial.clone(), new_identity.not_after);

    // The handle checks this retained snapshot under its shared writer lock.
    // Signing and validation stay outside that lock; a stale result is discarded
    // without touching the identity or its trust token and the next tick retries.
    if !handle.store_if_current(&expected, new_identity) {
        tracing::debug!(
            component = "conf_sync_ca",
            snapshot_serial = %old_serial,
            "another writer installed a fresher server identity during this tick; skipping the store"
        );
        return Ok(());
    }
    tracing::info!(
        component = "conf_sync_ca",
        event = "conf_sync_server_identity_resigned",
        reason,
        old_serial = %old_serial,
        new_serial = %new_serial,
        not_after = %not_after,
        "re-signed this replica's conf_sync server identity against the current root"
    );
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/unit/services/conf_sync_ca/identity_sync_test.rs"]
mod tests;
