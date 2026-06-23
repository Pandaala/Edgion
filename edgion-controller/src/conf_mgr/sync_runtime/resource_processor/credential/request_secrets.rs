//! Helper for resolving `${secretRef:ns/name:key}` inline secret references
//! in outbound request specs.
//!
//! Extracted from the Webhook arm of `link_sys.rs` so that other providers
//! (e.g. `forward_auth`) can reuse the same loop without duplicating the
//! fetch / error-mapping logic.

use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::Secret;

use edgion_resources::resources::common::SecretObjectReference;
use edgion_resources::resources::edgion_plugins::plugin_configs::parse_secret_ref;
use edgion_resources::ResourceKind;

use super::super::{HandlerContext, ResolvedRefsError, ResourceRef};
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
        match fetch_secret(ctx, owner, "edgion.io", owner_kind.as_str(), &secret_ref) {
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
mod tests {
    use super::*;

    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;

    use k8s_openapi::ByteString;
    use kube::api::ObjectMeta;

    use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResourceRef};
    use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::conf_mgr::ControllerRuntime;
    use edgion_resources::ResourceKind;

    fn make_ctx(secrets: HashMap<String, Secret>) -> (HandlerContext, Arc<ControllerRuntime>) {
        let runtime = Arc::new(ControllerRuntime::default());
        runtime.secret_store.replace_all(secrets);
        let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
        (ctx, runtime)
    }

    fn make_secret(namespace: &str, name: &str, data: &[(&str, &[u8])]) -> (String, Secret) {
        let mut bmap = BTreeMap::new();
        for (k, v) in data {
            bmap.insert(k.to_string(), ByteString(v.to_vec()));
        }
        let secret = Secret {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(namespace.to_string()),
                ..Default::default()
            },
            data: Some(bmap),
            ..Default::default()
        };
        (format!("{}/{}", namespace, name), secret)
    }

    fn owner() -> ResourceRef {
        ResourceRef::new(
            ResourceKind::LinkSys,
            Some("prod".to_string()),
            "my-webhook".to_string(),
        )
    }

    /// Build a `HandlerContext` pre-seeded with a single Secret.
    fn test_ctx_with_secret(ns_name: &str, key: &str, value: &str) -> HandlerContext {
        let (ns, name) = ns_name.split_once('/').expect("ns/name expected");
        let (secret_key, secret) = make_secret(ns, name, &[(key, value.as_bytes())]);
        let (ctx, _runtime) = make_ctx(HashMap::from([(secret_key, secret)]));
        ctx
    }

    /// A missing key in a present Secret surfaces as `DecodeError` (the Secret
    /// object exists; `read_secret_key` returns `Err` for the absent key).
    /// A missing Secret object surfaces as `SecretNotFound`.
    #[tokio::test]
    async fn resolve_request_secrets_reports_missing() {
        let ctx = test_ctx_with_secret("prod/s", "key", "v");
        let (map, errs) = resolve_request_secrets(
            &ctx,
            &owner(),
            ResourceKind::LinkSys,
            &["prod/s:key".to_string(), "prod/s:absent".to_string()],
        )
        .await;

        // The present key must resolve correctly.
        assert_eq!(map.get("prod/s:key").map(String::as_str), Some("v"));

        // The absent key surfaces as a DecodeError (key not found in secret).
        assert!(
            errs.iter().any(|e| matches!(e, ResolvedRefsError::DecodeError { .. })),
            "expected a DecodeError for the absent key, got: {errs:?}"
        );
    }

    #[tokio::test]
    async fn resolve_request_secrets_happy_path_two_keys() {
        let (key_a, secret_a) = make_secret("prod", "creds", &[("token", b"abc123"), ("secret", b"xyz789")]);
        let (ctx, _) = make_ctx(HashMap::from([(key_a, secret_a)]));

        let (map, errs) = resolve_request_secrets(
            &ctx,
            &owner(),
            ResourceKind::LinkSys,
            &["prod/creds:token".to_string(), "prod/creds:secret".to_string()],
        )
        .await;

        assert!(errs.is_empty(), "expected no errors, got: {errs:?}");
        assert_eq!(map.get("prod/creds:token").map(String::as_str), Some("abc123"));
        assert_eq!(map.get("prod/creds:secret").map(String::as_str), Some("xyz789"));
    }

    #[tokio::test]
    async fn resolve_request_secrets_missing_secret_returns_secret_not_found() {
        let (ctx, _) = make_ctx(HashMap::new());

        let (map, errs) =
            resolve_request_secrets(&ctx, &owner(), ResourceKind::LinkSys, &["prod/missing:key".to_string()]).await;

        assert!(map.is_empty());
        assert!(
            errs.iter()
                .any(|e| matches!(e, ResolvedRefsError::SecretNotFound { .. })),
            "expected SecretNotFound, got: {errs:?}"
        );
    }

    #[tokio::test]
    async fn resolve_request_secrets_malformed_ref_returns_decode_error() {
        let (ctx, _) = make_ctx(HashMap::new());

        // "nons:key" is malformed — missing the "namespace/" prefix required
        // by parse_secret_ref.
        let (map, errs) =
            resolve_request_secrets(&ctx, &owner(), ResourceKind::LinkSys, &["nons:key".to_string()]).await;

        assert!(map.is_empty());
        assert!(
            errs.iter()
                .any(|e| matches!(e, ResolvedRefsError::DecodeError { key: "secretRef", .. })),
            "expected DecodeError for malformed ref, got: {errs:?}"
        );
    }
}
