//! Helper for resolving `${secretRef:ns/name:key}` inline secret references
//! in outbound request specs.
//!
//! Extracted from the Webhook arm of `link_sys.rs` so that other providers
//! (e.g. `forward_auth`) can reuse the same loop without duplicating the
//! fetch / error-mapping logic.

use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::Secret;

use edgion_resources::resources::common::request_template::parse_secret_ref;
use edgion_resources::resources::common::SecretObjectReference;
use edgion_resources::ResourceKind;

use super::super::{HandlerContext, ResolvedRefsError, ResourceRef};
use super::pipeline::fetch_secret_readonly;
use super::{fetch_secret, FetchOutcome, PendingReason};

/// Read a single key out of a resolved K8s Secret as a UTF-8 string.
///
/// Checks `.data` (base64-decoded bytes) first, then `.string_data` as a
/// fallback (string_data is usually only present before the object is stored
/// server-side, but we handle it defensively). Returns an error string if the
/// key is absent or the bytes are not valid UTF-8.
pub(crate) fn read_secret_key(secret: &Secret, key: &str) -> Result<String, String> {
    if let Some(data) = &secret.data {
        if let Some(bytes) = data.get(key) {
            return String::from_utf8(bytes.0.clone()).map_err(|e| format!("key {:?}: invalid UTF-8: {}", key, e));
        }
    }
    if let Some(string_data) = &secret.string_data {
        if let Some(value) = string_data.get(key) {
            return Ok(value.clone());
        }
    }
    Err(format!("key {:?} not found in secret", key))
}

/// Resolve a slice of `"ns/name:key"` secret-ref names, returning a map of
/// successfully resolved values and a list of errors for any that could not
/// be resolved.
///
/// This is the shared loop body extracted from the Webhook handler in
/// `link_sys.rs`. It is parameterized by `owner_kind` so that future callers
/// (e.g. `forward_auth`) can provide the correct `from_kind` in
/// `RefNotPermitted` errors without hardcoding `"LinkSys"`.
///
/// The returned map is keyed by the raw `"ns/name:key"` string exactly as it
/// appears in the template (i.e. the same key that the gateway uses to look
/// up the resolved value at runtime).
pub async fn resolve_request_secrets(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    owner_kind: ResourceKind,
    names: &[String],
) -> (BTreeMap<String, String>, Vec<ResolvedRefsError>) {
    resolve_request_secrets_core(ctx, owner, owner_kind, names, true)
}

/// Re-evaluate request-template Secret refs for status without registering
/// dependencies again. Parse owns dependency registration; status derivation
/// must be a read-only, deterministic projection of current stores and grants.
pub fn request_secret_errors_readonly(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    owner_kind: ResourceKind,
    names: &[String],
) -> Vec<ResolvedRefsError> {
    resolve_request_secrets_core(ctx, owner, owner_kind, names, false).1
}

fn resolve_request_secrets_core(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    owner_kind: ResourceKind,
    names: &[String],
    register_dependencies: bool,
) -> (BTreeMap<String, String>, Vec<ResolvedRefsError>) {
    let mut resolved: BTreeMap<String, String> = BTreeMap::new();
    let mut errs: Vec<ResolvedRefsError> = Vec::new();

    for name in names {
        let parts = match parse_secret_ref(name) {
            Ok(p) => p,
            Err(e) => {
                // Malformed refs are also rejected at Accepted-validation;
                // record a resolved-refs error and skip.
                errs.push(ResolvedRefsError::DecodeError {
                    key: "secretRef",
                    detail: format!("{}: {}", name, e),
                });
                continue;
            }
        };
        let secret_ref = SecretObjectReference {
            group: None,
            kind: None,
            name: parts.name.clone(),
            namespace: Some(parts.namespace.clone()),
        };
        let outcome = if register_dependencies {
            fetch_secret(ctx, owner, "edgion.io", owner_kind.as_str(), &secret_ref)
        } else {
            fetch_secret_readonly(ctx, owner, "edgion.io", owner_kind.as_str(), &secret_ref)
        };
        match outcome {
            FetchOutcome::Ok(secret) => match read_secret_key(&secret, &parts.key) {
                Ok(value) => {
                    resolved.insert(name.clone(), value);
                }
                Err(detail) => errs.push(ResolvedRefsError::DecodeError {
                    key: "secretRef",
                    detail: format!("{}: {}", name, detail),
                }),
            },
            FetchOutcome::Pending(PendingReason::SecretNotFound {
                namespace,
                name: secret_name,
            }) => {
                errs.push(ResolvedRefsError::SecretNotFound {
                    namespace,
                    name: secret_name,
                });
            }
            FetchOutcome::Pending(PendingReason::ReferenceGrantMissing {
                from_ns,
                to_ns,
                secret_name,
            }) => {
                errs.push(ResolvedRefsError::RefNotPermitted {
                    from_namespace: from_ns,
                    from_kind: owner_kind.as_str(),
                    target_namespace: to_ns,
                    target_name: secret_name,
                });
            }
        }
    }

    (resolved, errs)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/credential/request_secrets_test.rs"]
mod tests;
