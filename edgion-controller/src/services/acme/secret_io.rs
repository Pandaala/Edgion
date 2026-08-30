//! ACME account Secret create/patch — uses `ACME_ACCOUNT_KEY` and merge-patch for self-managed Secrets.

use anyhow::{Context, Result};
use base64::Engine as _;
use instant_acme::AccountCredentials;
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use k8s_openapi::ByteString;
use kube::api::{Api, Patch, PatchParams};
use std::collections::BTreeMap;
use zeroize::Zeroizing;

use edgion_resources::credentials::ACME_ACCOUNT_KEY;

/// Legacy data key (pre-`ACME_ACCOUNT_KEY`); read-only for migration.
const LEGACY_ACME_ACCOUNT_KEY: &str = "credentials";

const EDGION_IO_MANAGED_BY: &str = "edgion.io/managed-by";
const EDGION_IO_ACME_RESOURCE: &str = "edgion.io/acme-resource";

/// Extract JSON bytes for the ACME account from Secret.data (prefers `accountCredentials`, else legacy `credentials`).
///
/// # Zeroize scope
///
/// See docs on [`edgion_resources::credentials::AcmeAccountMaterial`]. The
/// intermediate `String::from_utf8(raw.0.clone())` allocation below materializes
/// a plaintext copy of the account JSON that is **not** zero-wiped (only the
/// returned `Zeroizing<String>` is). Additionally, the raw `ByteString` inside
/// `data` remains plaintext in the caller-owned `Secret`. Callers should treat
/// the `Zeroizing` wrapper here as best-effort defense-in-depth.
pub fn extract_account_json_string(data: &BTreeMap<String, ByteString>) -> Result<Option<Zeroizing<String>>> {
    let entry = if let Some(b) = data.get(ACME_ACCOUNT_KEY) {
        Some((ACME_ACCOUNT_KEY, b))
    } else if let Some(b) = data.get(LEGACY_ACME_ACCOUNT_KEY) {
        tracing::warn!(
            key = %LEGACY_ACME_ACCOUNT_KEY,
            preferred = %ACME_ACCOUNT_KEY,
            "Deprecated ACME account Secret key: migrate data to accountCredentials"
        );
        Some((LEGACY_ACME_ACCOUNT_KEY, b))
    } else {
        None
    };

    let Some((key, raw)) = entry else {
        return Ok(None);
    };

    let s = String::from_utf8(raw.0.clone())
        .with_context(|| format!("ACME account Secret key {key:?} is not valid UTF-8"))?;
    Ok(Some(Zeroizing::new(s)))
}

/// Create the account Secret (server-side apply) with optional owner reference to the EdgionAcme.
pub async fn create_account_secret(
    secret_api: &Api<Secret>,
    secret_name: &str,
    namespace: &str,
    acme_resource_name: &str,
    acme_uid: Option<&str>,
    creds_json: &Zeroizing<String>,
) -> Result<()> {
    let mut meta = kube::api::ObjectMeta {
        name: Some(secret_name.to_string()),
        namespace: Some(namespace.to_string()),
        labels: Some(BTreeMap::from([
            (EDGION_IO_MANAGED_BY.to_string(), "acme".to_string()),
            (EDGION_IO_ACME_RESOURCE.to_string(), acme_resource_name.to_string()),
        ])),
        ..Default::default()
    };

    if let Some(uid) = acme_uid {
        meta.owner_references = Some(vec![OwnerReference {
            api_version: "edgion.io/v1".to_string(),
            kind: "EdgionAcme".to_string(),
            name: acme_resource_name.to_string(),
            uid: uid.to_string(),
            controller: Some(true),
            block_owner_deletion: Some(true),
        }]);
    } else {
        tracing::debug!(
            resource = %acme_resource_name,
            "EdgionAcme has no metadata.uid; creating account Secret without ownerReferences"
        );
    }

    let account_secret = Secret {
        metadata: meta,
        data: Some(BTreeMap::from([(
            ACME_ACCOUNT_KEY.to_string(),
            ByteString(creds_json.as_bytes().to_vec()),
        )])),
        type_: Some("Opaque".to_string()),
        ..Default::default()
    };

    // `force`: SSA only overwrites fields the same field manager already owns, so an
    // account Secret that was pre-created or ever `kubectl edit`ed belongs to another
    // manager and would 409 here on every attempt, permanently. The controller is the
    // sole authority for this Secret's content. (Guard: cicd/checks/check_ssa_force.py)
    let pp = PatchParams::apply("edgion-acme-controller").force();
    secret_api
        .patch(secret_name, &pp, &Patch::Apply(account_secret))
        .await
        .context("Failed to create/persist ACME account Secret")?;
    Ok(())
}

/// Merge-patch only the `ACME_ACCOUNT_KEY` entry, leaving other data keys and metadata intact (self-managed Secret).
pub async fn patch_account_key_into_existing_secret(
    secret_api: &Api<Secret>,
    secret_name: &str,
    creds_json: &Zeroizing<String>,
) -> Result<()> {
    let b64 = base64::engine::general_purpose::STANDARD.encode(creds_json.as_bytes());
    let patch = serde_json::json!({ "data": { ACME_ACCOUNT_KEY: b64 } });
    // No `.force()` here, deliberately: this is a Patch::Merge, and `force` is honored
    // only for Patch::Apply — a merge patch raises no field-ownership conflict at all.
    // `PatchParams::apply(...)` is used solely to set the field manager (kube exposes no
    // other setter). Allowlisted in cicd/checks/check_ssa_force.py with this reason.
    let pp = PatchParams::apply("edgion-acme-controller");
    secret_api
        .patch(secret_name, &pp, &Patch::Merge(patch))
        .await
        .context("Failed to patch ACME account credentials into existing Secret")?;
    Ok(())
}

/// Deserialize account credentials; `json` is zeroized on drop.
pub fn account_credentials_from_json(json: &Zeroizing<String>) -> Result<AccountCredentials> {
    serde_json::from_str(json.as_str()).context("EdgionAcme: account credentials are not valid instant_acme JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_prefers_account_credentials_over_legacy() {
        let mut data = BTreeMap::new();
        data.insert(
            ACME_ACCOUNT_KEY.to_string(),
            ByteString(br#"{"preferred":true}"#.to_vec()),
        );
        data.insert(
            LEGACY_ACME_ACCOUNT_KEY.to_string(),
            ByteString(br#"{"legacy":true}"#.to_vec()),
        );
        let z = extract_account_json_string(&data).unwrap().expect("Some");
        assert!(z.contains("preferred"));
    }

    #[test]
    fn extract_legacy_when_only_legacy_present() {
        let mut data = BTreeMap::new();
        data.insert(
            LEGACY_ACME_ACCOUNT_KEY.to_string(),
            ByteString(br#"{"only":"legacy"}"#.to_vec()),
        );
        let z = extract_account_json_string(&data).unwrap().expect("Some");
        assert!(z.contains("only"));
    }

    #[test]
    fn extract_none_when_no_keys() {
        let data = BTreeMap::new();
        assert!(extract_account_json_string(&data).unwrap().is_none());
    }
}
