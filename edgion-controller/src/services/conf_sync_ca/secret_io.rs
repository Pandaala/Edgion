//! Kubernetes Secret I/O for the conf_sync internal CA.
//!
//! Mirrors the server-side-apply pattern used by `services::acme::secret_io`:
//! `PatchParams::apply(...)` + `Patch::Apply` with `k8s_openapi::ByteString` data
//! and a managed-by label. These Secrets are NOT owned by a CRD, so there is no
//! `OwnerReference`.

use crate::services::conf_sync_ca::{Ca, LeafPem};
use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;
use kube::api::{Api, ObjectMeta, Patch, PatchParams, PostParams};
use std::collections::BTreeMap;

/// CA Secret: holds `ca.crt` + `ca.key`. Controller-only RBAC; the gateway SA must
/// not be able to read it.
pub const CA_SECRET: &str = "edgion-conf-sync-ca";
/// Gateway leaf Secret: holds `client.crt` + `client.key` + `ca.crt`. Mounted by the
/// gateway pod. Controller-owned (overwritten to stay consistent with the active CA).
pub const LEAF_SECRET: &str = "edgion-conf-sync-certs";

const MANAGED_BY: &str = "edgion.io/managed-by";
const FIELD_MANAGER: &str = "edgion-conf-sync-ca";

/// Pure builder: an Opaque Secret with the managed-by label and the given data.
pub fn build_secret(name: &str, ns: &str, data: BTreeMap<String, Vec<u8>>) -> Secret {
    Secret {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(ns.to_string()),
            labels: Some(BTreeMap::from([(MANAGED_BY.to_string(), "conf-sync-ca".to_string())])),
            ..Default::default()
        },
        data: Some(data.into_iter().map(|(k, v)| (k, ByteString(v))).collect()),
        type_: Some("Opaque".to_string()),
        ..Default::default()
    }
}

/// Pure builder for the CA Secret data map (`ca.crt` + `ca.key`). Shared by the
/// create path in [`ensure_ca`] and the renewal path in [`write_ca`] so both
/// always carry BOTH fields — a renewal that dropped `ca.key` would orphan the
/// root key and break all future re-signing.
fn ca_secret_data(ca: &Ca) -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([
        ("ca.crt".to_string(), ca.ca_cert_pem().as_bytes().to_vec()),
        ("ca.key".to_string(), ca.ca_key_pem().as_bytes().to_vec()),
    ])
}

/// Server-side-apply params for the Secrets this module owns.
///
/// `force` is REQUIRED, not optional hygiene: server-side apply only overwrites
/// fields the *same* field manager already owns. Every other owner triggers a 409
/// Conflict instead — and both of our Secrets routinely have another owner:
///
/// - `edgion-conf-sync-ca` is CREATED by [`ensure_ca`]'s POST, so the API server
///   records the fields under the client's default manager (`unknown`), not
///   [`FIELD_MANAGER`]. A CA-renewal apply without `force` therefore 409s on every
///   attempt, permanently — the root would expire with the reconciler stuck in
///   backoff.
/// - `edgion-conf-sync-certs` is normally apply-created by us, but an operator
///   `kubectl create/edit/patch` (or the documented "adopt a manually created
///   leaf" upgrade path) hands field ownership to kubectl and would likewise
///   wedge every future rotation.
///
/// Forcing is the correct semantic here: the controller is the sole authority for
/// the content of both Secrets, and convergence between replicas is guaranteed by
/// [`ensure_ca`]'s POST-and-adopt, not by apply conflicts.
fn apply_params() -> PatchParams {
    PatchParams::apply(FIELD_MANAGER).force()
}

/// Extract a UTF-8 data value from a Secret (PEM strings are ASCII).
fn data_str(s: &Secret, key: &str) -> Option<String> {
    s.data
        .as_ref()?
        .get(key)
        .map(|b| String::from_utf8_lossy(&b.0).to_string())
}

/// READ-ONLY CA load. GETs the CA Secret and returns the adopted root, or `None`
/// when the Secret is absent or does not carry both `ca.crt` and `ca.key`.
///
/// **This function NEVER creates anything.** That is the whole point of it
/// existing next to [`ensure_ca`]:
///
/// - [`ensure_ca`] is a WRITE path — on absence it POST-creates a brand-new root.
///   After startup, the elected LEADER's rotation reconciler is its only caller
///   (see the bootstrap exception documented on [`ensure_ca`]).
/// - [`load_ca`] is the READ path used by every replica's steady-state loops
///   (leader and non-leader alike; see `services::conf_sync_ca::identity_sync`). A
///   periodic non-leader loop that called `ensure_ca` would POST a second,
///   unrelated root whenever the Secret happened to be absent — split-braining the
///   trust anchor for the rest of the fleet's uptime.
///
/// So: absence is reported as `None` (the caller waits for the leader), never
/// papered over by minting a root.
pub async fn load_ca(secret_api: &Api<Secret>) -> Result<Option<Ca>> {
    let Some(existing) = secret_api.get_opt(CA_SECRET).await.context("get CA secret")? else {
        return Ok(None);
    };
    let (Some(crt), Some(key)) = (data_str(&existing, "ca.crt"), data_str(&existing, "ca.key")) else {
        return Ok(None);
    };
    Ok(Some(Ca::load_from_pem(&crt, &key).context("load existing CA secret")?))
}

/// Get-or-create-or-adopt the CA. **WRITE PATH.**
///
/// - If `edgion-conf-sync-ca` exists with `ca.crt` + `ca.key`, adopt it (BYO-root).
/// - Otherwise generate a fresh root and POST-create it.
/// - On a concurrent create (HTTP 409 / `AlreadyExists`), re-read and adopt the
///   winner's root (leader race / idempotency).
///
/// # Who may call this
///
/// In **steady state**, the elected leader's rotation reconciler is the single
/// writer. A periodic non-leader loop must NOT call this — it would mint a rival
/// root during any window where the Secret is absent. Use the read-only [`load_ca`]
/// there (that is exactly what `identity_sync` does).
///
/// **The deliberate bootstrap exception**: `provision()` (`provision.rs`) calls this
/// — and [`write_leaf`] — from **every** replica at startup, before leadership is
/// known (`cli/mod.rs` calls `provision()` unconditionally). That is load-bearing:
/// a replica cannot serve conf_sync mTLS at all until it holds a root, and it must
/// not have to wait for a leader election to start. It is safe for the CA half
/// precisely because of the POST-and-adopt above: concurrent bootstrappers race on a
/// real HTTP 409, exactly one root survives, and the losers adopt it. An SSA apply
/// here would be last-writer-wins and would NOT 409 — which is why the create path
/// deliberately uses POST.
///
/// The leaf half of that bootstrap has a **known, benign race**: [`write_leaf`] is a
/// FORCED apply (last-writer-wins), so if two replicas boot concurrently while the
/// leaf Secret is missing or past its emergency window (a rolling restart right
/// after a re-key, a node drain), each mints a *different* client leaf and force-
/// applies it — two consecutive gateway Secret swaps, hence two fleet-wide conf_sync
/// channel rebuilds. Nothing is compromised: both leaves are valid and chain to the
/// same (single, 409-arbitrated) root, and the gateways converge on whichever landed
/// last. The cost is churn, not a split trust anchor. `provision()` narrows the
/// window with a re-read immediately before the write; it does not close it, and it
/// does not need to.
pub async fn ensure_ca(secret_api: &Api<Secret>, ns: &str, ca_ttl: time::Duration) -> Result<Ca> {
    if let Some(existing) = secret_api.get_opt(CA_SECRET).await.context("get CA secret")? {
        if let (Some(crt), Some(key)) = (data_str(&existing, "ca.crt"), data_str(&existing, "ca.key")) {
            return Ca::load_from_pem(&crt, &key).context("adopt existing CA secret");
        }
    }

    let ca = Ca::generate_root(ca_ttl)?;
    let secret = build_secret(CA_SECRET, ns, ca_secret_data(&ca));
    // Use create() (POST), NOT server-side apply: a second apply with the same field
    // manager is last-writer-wins and would NOT 409, letting two replicas persist
    // different roots. A POST yields a real AlreadyExists/409, so the loser re-reads
    // and adopts the winner's root — all replicas converge on one CA.
    match secret_api.create(&PostParams::default(), &secret).await {
        Ok(_) => Ok(ca),
        Err(kube::Error::Api(ae)) if ae.code == 409 => {
            // Someone created it first — re-read and adopt theirs.
            let existing = secret_api.get(CA_SECRET).await.context("re-read CA secret after 409")?;
            let crt = data_str(&existing, "ca.crt").context("CA secret missing ca.crt")?;
            let key = data_str(&existing, "ca.key").context("CA secret missing ca.key")?;
            Ca::load_from_pem(&crt, &key)
        }
        Err(e) => Err(e).context("create CA secret"),
    }
}

/// SSA-write the controller-owned gateway leaf Secret (client cert/key + ca.crt).
/// Overwrites any existing content to keep the leaf consistent with the active CA.
///
/// **Forced apply = last-writer-wins.** In steady state the elected leader is the
/// only caller, so that is unambiguous. The one exception is startup `provision()`,
/// which every replica runs — see the "known, benign race" paragraph on [`ensure_ca`].
pub async fn write_leaf(secret_api: &Api<Secret>, ns: &str, leaf: &LeafPem, ca_cert_pem: &str) -> Result<()> {
    let data = BTreeMap::from([
        ("client.crt".to_string(), leaf.cert_pem.as_bytes().to_vec()),
        ("client.key".to_string(), leaf.key_pem.as_bytes().to_vec()),
        ("ca.crt".to_string(), ca_cert_pem.as_bytes().to_vec()),
    ]);
    let secret = build_secret(LEAF_SECRET, ns, data);
    secret_api
        .patch(LEAF_SECRET, &apply_params(), &Patch::Apply(secret))
        .await
        .context("apply gateway leaf secret")?;
    Ok(())
}

/// SSA-write the renewed root CA back to the CA Secret (`ca.crt` + `ca.key`).
///
/// Used by the rotation reconciler's `RotateScope::Ca` path after
/// `renew_root_same_key`: the certificate PEM changes but the key PEM is the
/// SAME bytes, so `ca.key` MUST be re-applied alongside `ca.crt` (both come from
/// [`ca_secret_data`]) or a future `ensure_ca`/re-sign would find the key gone.
///
/// The apply is forced ([`apply_params`]): the Secret was POST-created by
/// [`ensure_ca`], so its fields belong to another manager and an unforced apply
/// would 409 on every renewal. Convergence between replicas is not at stake here
/// — the caller is the elected leader re-signing the root it already holds.
pub async fn write_ca(secret_api: &Api<Secret>, ns: &str, ca: &Ca) -> Result<()> {
    let secret = build_secret(CA_SECRET, ns, ca_secret_data(ca));
    secret_api
        .patch(CA_SECRET, &apply_params(), &Patch::Apply(secret))
        .await
        .context("apply renewed CA secret")?;
    Ok(())
}

/// Read the gateway leaf Secret if present: (client.crt, client.key, ca.crt).
pub async fn read_leaf(secret_api: &Api<Secret>) -> Result<Option<(String, String, String)>> {
    let Some(s) = secret_api.get_opt(LEAF_SECRET).await.context("get leaf secret")? else {
        return Ok(None);
    };
    Ok(
        match (
            data_str(&s, "client.crt"),
            data_str(&s, "client.key"),
            data_str(&s, "ca.crt"),
        ) {
            (Some(c), Some(k), Some(a)) => Some((c, k, a)),
            _ => None,
        },
    )
}

#[cfg(test)]
#[path = "../../../tests/unit/services/conf_sync_ca/secret_io_test.rs"]
mod tests;
