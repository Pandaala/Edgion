//! Startup provisioning for the conf_sync internal CA.
//!
//! Ensures the CA exists (adopt or self-generate), provisions the gateway-client
//! leaf Secret, and mints the controller-server cert IN MEMORY (never a Secret) so
//! the controller does not depend on a mounted cert Secret to start.

use super::secret_io::{ensure_ca, read_leaf, write_leaf};
use anyhow::Result;
use edgion_common::conf_sync::config::ConfSyncRotationConfig;
use k8s_openapi::api::core::v1::Secret;
use kube::{Api, Client};
use zeroize::Zeroizing;

/// In-memory controller-server identity plus the CA bundle advertised to clients.
pub struct ServerIdentityPem {
    pub cert_pem: String,
    pub key_pem: Zeroizing<String>,
    pub ca_cert_pem: String,
}

/// Pure adopt-or-rewrite decision for the gateway leaf Secret (spec §6 T2
/// prerequisite): rewrite only when missing, unparsable, not chaining to the
/// adopted CA, or within the emergency window. Everything else is adopted so
/// replica restarts / rolling updates do not churn gateway certs.
pub fn leaf_needs_rewrite(
    existing: Option<(&str, &str)>,
    ca_pem: &str,
    emergency: time::Duration,
    now: time::OffsetDateTime,
) -> bool {
    let Some((cert, key)) = existing else { return true };
    if edgion_common::grpc_tls::validate_identity(cert.as_bytes(), key.as_bytes(), ca_pem.as_bytes()).is_err() {
        return true;
    }
    match super::cert_util::parse_cert_window(cert) {
        Ok((_, na)) => na - now <= emergency,
        Err(_) => true,
    }
}

/// Ensure the CA, adopt-or-(re)provision the gateway-client leaf Secret, and mint
/// the controller-server cert in memory. Idempotent (safe to run on every
/// startup; the CA POST-create adopts on a 409 race). TTLs come from
/// `rotation.leaf_ttl`/`rotation.ca_ttl`. The in-memory server leaf is minted
/// fresh on every call regardless of the leaf-Secret adopt decision.
///
/// **This is the deliberate bootstrap exception to "the leader is the single
/// writer"**: `cli/mod.rs` calls it on EVERY replica at startup, before leadership
/// is known, because a replica cannot serve conf_sync mTLS until it holds a root and
/// must not wait for an election to start. The CA half is race-safe by construction
/// (POST + 409-adopt). The leaf half is a forced apply whose concurrent-boot race is
/// benign (churn, not compromise) and is narrowed by the re-read below — see
/// `secret_io::ensure_ca` for the full argument.
pub async fn provision(
    client: Client,
    ns: &str,
    server_sans: Vec<String>,
    rotation: &ConfSyncRotationConfig,
) -> Result<ServerIdentityPem> {
    let secret_api: Api<Secret> = Api::namespaced(client, ns);
    let ca = ensure_ca(&secret_api, ns, rotation.ca_ttl.as_time()).await?;

    // Gateway client leaf → Secret, adopted unless it needs rewrite (spec §6 T2).
    let existing = read_leaf(&secret_api).await?;
    let now = time::OffsetDateTime::now_utc();
    if leaf_needs_rewrite(
        existing.as_ref().map(|(c, k, _)| (c.as_str(), k.as_str())),
        ca.ca_cert_pem(),
        rotation.emergency_rotate_below.as_time(),
        now,
    ) {
        // Re-read once, immediately before writing.
        //
        // `provision()` runs on EVERY replica at startup (before leadership is known),
        // and `write_leaf` is a FORCED apply — last-writer-wins. Two replicas booting
        // concurrently into a "needs rewrite" state (leaf Secret missing or inside its
        // emergency window: a rolling restart right after a re-key, a node drain) would
        // each mint a different leaf and force-apply it, causing two consecutive gateway
        // Secret swaps and therefore two fleet-wide conf_sync channel rebuilds — exactly
        // while an operator is trying to read whether the re-key landed.
        //
        // Between our first read above and here we have done a CA round-trip and, on the
        // decision path, nothing else; the other replica has usually landed its write in
        // that gap. Re-running the same check on fresh bytes lets us adopt its healthy
        // leaf instead of clobbering it.
        //
        // This is a cheap read-then-act NARROWING, not a lock: it shrinks the race window
        // from "the whole provision() prologue" to "one GET + one sign", it does not close
        // it, and it does not need to — a double write is churn, never a split trust
        // anchor (both leaves chain to the single 409-arbitrated root; see
        // `secret_io::ensure_ca`).
        let fresh = read_leaf(&secret_api).await?;
        if !leaf_needs_rewrite(
            fresh.as_ref().map(|(c, k, _)| (c.as_str(), k.as_str())),
            ca.ca_cert_pem(),
            rotation.emergency_rotate_below.as_time(),
            time::OffsetDateTime::now_utc(),
        ) {
            tracing::info!(
                component = "conf_sync_ca",
                ns = %ns,
                "another replica provisioned a healthy gateway leaf Secret concurrently; adopting it"
            );
        } else {
            let client_leaf = ca.sign_client(rotation.leaf_ttl.as_time())?;
            write_leaf(&secret_api, ns, &client_leaf, ca.ca_cert_pem()).await?;
            tracing::info!(component = "conf_sync_ca", ns = %ns, "gateway leaf Secret (re)provisioned");
        }
    } else {
        tracing::info!(component = "conf_sync_ca", ns = %ns, "existing gateway leaf Secret adopted");
    }

    // Controller server cert → in memory (never persisted as a Secret); minted
    // fresh every start regardless of the leaf-Secret adopt decision above.
    let server_leaf = ca.sign_server(&server_sans, rotation.leaf_ttl.as_time())?;
    Ok(ServerIdentityPem {
        cert_pem: server_leaf.cert_pem,
        key_pem: server_leaf.key_pem,
        ca_cert_pem: ca.ca_cert_pem().to_string(),
    })
}

/// Build the controller-server DNS SANs from the namespace, mirroring
/// `edgion-cli gen-certs` so the gateway's hostname verification passes.
pub fn server_sans_for(ns: &str) -> Vec<String> {
    vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
        // The gateway connects via the leader Service (edgion-controller-leader),
        // which selects the elected leader pod. Hostname verification is done
        // against THIS name, so it must be in the SAN set.
        format!("edgion-controller-leader.{ns}.svc.cluster.local"),
        format!("edgion-controller-leader.{ns}.svc"),
        "edgion-controller-leader".to_string(),
        // Headless / direct Service names (kept for completeness / non-leader access).
        format!("edgion-controller.{ns}.svc.cluster.local"),
        format!("edgion-controller.{ns}.svc"),
        "edgion-controller".to_string(),
    ]
}

#[cfg(test)]
#[path = "../../../tests/unit/services/conf_sync_ca/provision_test.rs"]
mod tests;
