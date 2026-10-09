//! Secret acquisition and readonly status from actual owning request-template fields.

use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::Secret;

use edgion_resources::resources::refs::TemplateSecretReference;

use super::super::{HandlerContext, ResolvedRefsError, ResourceRef};
use super::pipeline::fetch_bound_secret_readonly;
use super::{fetch_bound_secret, BoundFetchOutcome, PendingReason};

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

/// Preserve exact runtime map keys while acquiring only actual owning tokens.
pub fn resolve_request_secrets<'a>(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    references: impl IntoIterator<Item = TemplateSecretReference<'a>>,
) -> (BTreeMap<String, String>, Vec<ResolvedRefsError>) {
    resolve_request_secrets_core(ctx, owner, references, true)
}

/// Canonical acquisition for a declaring field whose root recovery receipts were
/// installed separately. This performs no dependency registration.
pub(crate) fn resolve_request_secrets_readonly<'a>(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    references: impl IntoIterator<Item = TemplateSecretReference<'a>>,
) -> (BTreeMap<String, String>, Vec<ResolvedRefsError>) {
    resolve_request_secrets_core(ctx, owner, references, false)
}

/// Re-evaluate request-template Secret refs for status without registering
/// dependencies again. Parse owns dependency registration; status derivation
/// must be a read-only, deterministic projection of current stores and grants.
pub fn request_secret_errors_readonly<'a>(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    references: impl IntoIterator<Item = TemplateSecretReference<'a>>,
) -> Vec<ResolvedRefsError> {
    resolve_request_secrets_core(ctx, owner, references, false).1
}

fn resolve_request_secrets_core<'a>(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    references: impl IntoIterator<Item = TemplateSecretReference<'a>>,
    register_dependencies: bool,
) -> (BTreeMap<String, String>, Vec<ResolvedRefsError>) {
    let mut resolved: BTreeMap<String, String> = BTreeMap::new();
    let mut errs: Vec<ResolvedRefsError> = Vec::new();

    for reference in references {
        let token = *reference.reference();
        if reference.data_key().is_empty() {
            errs.push(ResolvedRefsError::DecodeError {
                key: "secretRef",
                detail: "invalid request-template Secret reference".to_string(),
            });
            continue;
        }
        let outcome = if register_dependencies {
            fetch_bound_secret(ctx, owner, token)
        } else {
            fetch_bound_secret_readonly(ctx, owner, token)
        };
        match outcome {
            BoundFetchOutcome::Ok(authorized) => match read_secret_key(&authorized.secret, reference.data_key()) {
                Ok(value) => {
                    resolved.insert(reference.map_key().to_string(), value);
                }
                Err(detail) => errs.push(ResolvedRefsError::DecodeError {
                    key: "secretRef",
                    detail: format!("{}: {}", reference.map_key(), detail),
                }),
            },
            BoundFetchOutcome::Invalid(error) => errs.push(ResolvedRefsError::InvalidReference { error }),
            BoundFetchOutcome::Pending(PendingReason::SecretNotFound {
                namespace,
                name: secret_name,
            }) => {
                errs.push(ResolvedRefsError::SecretNotFound {
                    namespace,
                    name: secret_name,
                });
            }
            BoundFetchOutcome::Pending(PendingReason::ReferenceGrantMissing {
                from_ns,
                to_ns,
                secret_name,
            }) => {
                errs.push(ResolvedRefsError::RefNotPermitted {
                    from_namespace: from_ns,
                    from_kind: owner.kind_str(),
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
