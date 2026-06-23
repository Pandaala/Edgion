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

/// Extract a UTF-8 data value from a Secret (PEM strings are ASCII).
fn data_str(s: &Secret, key: &str) -> Option<String> {
    s.data
        .as_ref()?
        .get(key)
        .map(|b| String::from_utf8_lossy(&b.0).to_string())
}

/// Get-or-create-or-adopt the CA.
///
/// - If `edgion-conf-sync-ca` exists with `ca.crt` + `ca.key`, adopt it (BYO-root).
/// - Otherwise generate a fresh root and SSA-create it.
/// - On a concurrent create (HTTP 409 / `AlreadyExists`), re-read and adopt the
///   winner's root (leader race / idempotency).
pub async fn ensure_ca(secret_api: &Api<Secret>, ns: &str, ca_days: u32) -> Result<Ca> {
    if let Some(existing) = secret_api.get_opt(CA_SECRET).await.context("get CA secret")? {
        if let (Some(crt), Some(key)) = (data_str(&existing, "ca.crt"), data_str(&existing, "ca.key")) {
            return Ca::load_from_pem(&crt, &key).context("adopt existing CA secret");
        }
    }

    let ca = Ca::generate_root(ca_days)?;
    let data = BTreeMap::from([
        ("ca.crt".to_string(), ca.ca_cert_pem().as_bytes().to_vec()),
        ("ca.key".to_string(), ca.ca_key_pem().as_bytes().to_vec()),
    ]);
    let secret = build_secret(CA_SECRET, ns, data);
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
pub async fn write_leaf(secret_api: &Api<Secret>, ns: &str, leaf: &LeafPem, ca_cert_pem: &str) -> Result<()> {
    let data = BTreeMap::from([
        ("client.crt".to_string(), leaf.cert_pem.as_bytes().to_vec()),
        ("client.key".to_string(), leaf.key_pem.as_bytes().to_vec()),
        ("ca.crt".to_string(), ca_cert_pem.as_bytes().to_vec()),
    ]);
    let secret = build_secret(LEAF_SECRET, ns, data);
    let pp = PatchParams::apply(FIELD_MANAGER);
    secret_api
        .patch(LEAF_SECRET, &pp, &Patch::Apply(secret))
        .await
        .context("apply gateway leaf secret")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn build_secret_sets_name_ns_label_and_data() {
        let mut data = BTreeMap::new();
        data.insert("ca.crt".to_string(), b"X".to_vec());
        let s = build_secret("edgion-conf-sync-ca", "edgion-system", data);
        assert_eq!(s.metadata.name.as_deref(), Some("edgion-conf-sync-ca"));
        assert_eq!(s.metadata.namespace.as_deref(), Some("edgion-system"));
        assert!(s.metadata.labels.as_ref().unwrap().contains_key("edgion.io/managed-by"));
        assert!(s.data.as_ref().unwrap().contains_key("ca.crt"));
        assert_eq!(s.type_.as_deref(), Some("Opaque"));
    }
}
