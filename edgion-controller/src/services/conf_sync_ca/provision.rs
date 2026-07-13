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
/// startup; the CA SSA-create adopts on a 409 race). TTLs come from
/// `rotation.leaf_ttl`/`rotation.ca_ttl`. The in-memory server leaf is minted
/// fresh on every call regardless of the leaf-Secret adopt decision.
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
        let client_leaf = ca.sign_client(rotation.leaf_ttl.as_time())?;
        write_leaf(&secret_api, ns, &client_leaf, ca.ca_cert_pem()).await?;
        tracing::info!(component = "conf_sync_ca", ns = %ns, "gateway leaf Secret (re)provisioned");
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
mod tests {
    use super::*;

    #[test]
    fn server_sans_include_leader_service_fqdn_and_loopback() {
        let sans = server_sans_for("edgion-system");
        assert!(sans.contains(&"127.0.0.1".to_string()));
        // The leader-Service FQDN is the host the gateway verifies against — it MUST
        // be present or every mTLS handshake fails hostname verification.
        assert!(sans.contains(&"edgion-controller-leader.edgion-system.svc.cluster.local".to_string()));
        // The direct Service name is kept too.
        assert!(sans.contains(&"edgion-controller.edgion-system.svc.cluster.local".to_string()));
    }

    #[test]
    fn leaf_needs_rewrite_decision_table() {
        use crate::services::conf_sync_ca::cert_util::BACKDATE_SLACK;
        use crate::services::conf_sync_ca::Ca;
        let _ = BACKDATE_SLACK; // silence if unused
        let ca = Ca::generate_root(time::Duration::days(365)).unwrap();
        let leaf = ca.sign_client(time::Duration::days(30)).unwrap();
        let now = time::OffsetDateTime::now_utc();
        let emergency = time::Duration::hours(24);
        // healthy existing leaf -> adopt
        assert!(!leaf_needs_rewrite(
            Some((&leaf.cert_pem, leaf.key_pem.as_str())),
            ca.ca_cert_pem(),
            emergency,
            now
        ));
        // missing -> rewrite
        assert!(leaf_needs_rewrite(None, ca.ca_cert_pem(), emergency, now));
        // garbage -> rewrite
        assert!(leaf_needs_rewrite(
            Some(("bad", "bad")),
            ca.ca_cert_pem(),
            emergency,
            now
        ));
        // wrong CA (stale BYO-root leaf) -> rewrite
        let other = Ca::generate_root(time::Duration::days(365)).unwrap();
        assert!(leaf_needs_rewrite(
            Some((&leaf.cert_pem, leaf.key_pem.as_str())),
            other.ca_cert_pem(),
            emergency,
            now
        ));
        // near expiry -> rewrite (1-day leaf, 25h emergency threshold: the leaf's
        // entire remaining validity is inside the emergency window)
        let dying = ca.sign_client(time::Duration::days(1)).unwrap();
        assert!(leaf_needs_rewrite(
            Some((&dying.cert_pem, dying.key_pem.as_str())),
            ca.ca_cert_pem(),
            time::Duration::hours(25),
            now
        ));
    }
}
