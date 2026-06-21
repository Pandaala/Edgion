//! Startup provisioning for the conf_sync internal CA.
//!
//! Ensures the CA exists (adopt or self-generate), provisions the gateway-client
//! leaf Secret, and mints the controller-server cert IN MEMORY (never a Secret) so
//! the controller does not depend on a mounted cert Secret to start.

use super::secret_io::{ensure_ca, write_leaf};
use anyhow::Result;
use k8s_openapi::api::core::v1::Secret;
use kube::{Api, Client};
use zeroize::Zeroizing;

/// In-memory controller-server identity plus the CA bundle advertised to clients.
pub struct ServerIdentityPem {
    pub cert_pem: String,
    pub key_pem: Zeroizing<String>,
    pub ca_cert_pem: String,
}

/// Root CA validity (long-lived; leaves rotate in Phase 2).
const ROOT_DAYS: u32 = 3650;
/// Leaf validity (1 year for v0; Phase 2 adds 50%-lifetime rotation).
const LEAF_DAYS: u32 = 365;

/// Ensure the CA, provision the gateway-client leaf Secret, and mint the
/// controller-server cert in memory. Idempotent (safe to run on every startup;
/// the CA SSA-create adopts on a 409 race).
pub async fn provision(client: Client, ns: &str, server_sans: Vec<String>) -> Result<ServerIdentityPem> {
    let secret_api: Api<Secret> = Api::namespaced(client, ns);
    let ca = ensure_ca(&secret_api, ns, ROOT_DAYS).await?;

    // Gateway client leaf → Secret (controller-owned, overwrite to track the CA).
    let client_leaf = ca.sign_client(LEAF_DAYS)?;
    write_leaf(&secret_api, ns, &client_leaf, ca.ca_cert_pem()).await?;

    // Controller server cert → in memory (never persisted as a Secret).
    let server_leaf = ca.sign_server(&server_sans, LEAF_DAYS)?;
    tracing::info!(
        component = "conf_sync_ca",
        ns = %ns,
        "conf_sync auto-CA provisioned CA + gateway leaf Secret + in-memory server cert"
    );
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
}
