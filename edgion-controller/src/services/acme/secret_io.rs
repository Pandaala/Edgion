//! ACME account Secret create/patch — uses `ACME_ACCOUNT_KEY` and merge-patch for self-managed Secrets.

use anyhow::{Context, Result};
use base64::Engine as _;
use edgion_resources::resources::refs::acme_fields::AcmeAccountField;
use instant_acme::AccountCredentials;
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use k8s_openapi::ByteString;
use kube::api::{Api, Patch, PatchParams};
use kube::Client;
use std::collections::BTreeMap;
use zeroize::Zeroizing;

use edgion_resources::credentials::ACME_ACCOUNT_KEY;

const EDGION_IO_MANAGED_BY: &str = "edgion.io/managed-by";
const EDGION_IO_ACME_RESOURCE: &str = "edgion.io/acme-resource";

/// Extract JSON bytes for the ACME account from Secret.data.
///
/// # Zeroize scope
///
/// See docs on [`edgion_resources::credentials::AcmeAccountMaterial`]. The
/// cloned byte buffer below is moved into the decoded `String`, then into the
/// returned `Zeroizing<String>` without another allocation. That successful
/// decode buffer is wiped on wrapper drop. On invalid UTF-8, the failed buffer
/// belongs to the decode error and is not wiped. The original `ByteString` in
/// the caller-owned `Secret` also remains unprotected. This is local
/// defense-in-depth, not an end-to-end erasure guarantee.
pub fn extract_account_json_string(data: &BTreeMap<String, ByteString>) -> Result<Option<Zeroizing<String>>> {
    let Some(raw) = data.get(ACME_ACCOUNT_KEY) else {
        return Ok(None);
    };

    let s = String::from_utf8(raw.0.clone())
        .with_context(|| format!("ACME account Secret key {ACME_ACCOUNT_KEY:?} is not valid UTF-8"))?;
    Ok(Some(Zeroizing::new(s)))
}

/// Create the account Secret with an owner reference only to a same-namespace EdgionAcme.
pub async fn create_account_secret(
    client: &Client,
    field: &AcmeAccountField<'_>,
    creds_json: &Zeroizing<String>,
) -> Result<()> {
    let token = field.token();
    let reference = token.validated()?;
    let target = reference.target();
    let secret_name = target.name();
    let namespace = target
        .scope()
        .namespace()
        .context("ACME account Secret namespace is missing")?;
    let secret_api: Api<Secret> = Api::namespaced(client.clone(), namespace);
    let source = field.owner();
    let acme_resource_name = source.metadata.name.as_deref().context("EdgionAcme has no name")?;
    let acme_uid = source.metadata.uid.as_deref();
    let mut meta = kube::api::ObjectMeta {
        name: Some(secret_name.to_string()),
        namespace: Some(namespace.to_string()),
        labels: Some(BTreeMap::from([
            (EDGION_IO_MANAGED_BY.to_string(), "acme".to_string()),
            (EDGION_IO_ACME_RESOURCE.to_string(), acme_resource_name.to_string()),
        ])),
        ..Default::default()
    };

    // Kubernetes cannot represent ownership by an object in another namespace.
    if let Some(uid) = acme_uid.filter(|_| reference.source().scope() == target.scope()) {
        meta.owner_references = Some(vec![OwnerReference {
            api_version: "edgion.io/v1".to_string(),
            kind: "EdgionAcme".to_string(),
            name: acme_resource_name.to_string(),
            uid: uid.to_string(),
            controller: Some(true),
            block_owner_deletion: Some(true),
        }]);
    } else if acme_uid.is_none() {
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
    client: &Client,
    field: &AcmeAccountField<'_>,
    snapshot: &Secret,
    creds_json: &Zeroizing<String>,
) -> Result<()> {
    field
        .validate_api_secret(snapshot)
        .context("ACME account Secret does not match its declared reference")?;
    let token = field.token();
    let target = token.validated()?.target();
    let secret_name = target.name();
    let namespace = target
        .scope()
        .namespace()
        .context("ACME account Secret namespace is missing")?;
    let secret_api: Api<Secret> = Api::namespaced(client.clone(), namespace);
    let uid = snapshot
        .metadata
        .uid
        .as_deref()
        .context("ACME account Secret has no UID")?;
    let resource_version = snapshot
        .metadata
        .resource_version
        .as_deref()
        .context("ACME account Secret has no resourceVersion")?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(creds_json.as_bytes());
    let patch = serde_json::json!({
        "metadata": {"uid": uid, "resourceVersion": resource_version},
        "data": { ACME_ACCOUNT_KEY: b64 }
    });
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
#[path = "../../../tests/unit/services/acme/secret_io_test.rs"]
mod tests;
