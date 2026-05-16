//! LinkSys Handler
//!
//! Handles LinkSys resources with:
//! - Validation (preparse hook)
//! - SecretRef resolution via `resolve_secret` pipeline (attaches Secret to spec)
//! - SecretRefManager registration for cascading updates
//! - Gateway API standard status management
//!
//! Note: the resolved Secret is embedded into the corresponding `*Auth.secret` field,
//! which is serialized across conf_sync to the Gateway. Material parsing happens on
//! the Gateway side via `crate::types::credentials`.

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::credential::{
    resolve_secret, InvalidCredentialError, PendingReason, ResolveOutcome,
};
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    accepted_condition, condition_false, condition_types, resolved_refs_condition, update_condition, HandlerContext,
    ProcessResult, ProcessorHandler, ResolvedRefsError, ResourceRef,
};
use crate::core::gateway::backends::service_kind::EdgionService;
use crate::types::credentials::SecretMaterialResolver;
use crate::types::prelude_resources::LinkSys;
use crate::types::resources::common::SecretObjectReference;
use crate::types::resources::link_sys::webhook::WebhookTarget;
use crate::types::resources::link_sys::{LinkSysStatus, SystemConfig};

use super::auth_spec::{AuthSpec, TlsSpec};
use crate::types::ResourceKind;
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::api::discovery::v1::EndpointSlice;

/// LinkSys handler
pub struct LinkSysHandler;

impl LinkSysHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LinkSysHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Translate a pipeline `ResolveOutcome<T>` into `Result<T, ResolvedRefsError>`.
///
/// Cross-namespace denial maps to the Gateway-API standard `RefNotPermitted`
/// (not a dedicated `ReferenceGrantMissing` reason): the pipeline cannot
/// distinguish "no grant yet" from "grant denies", so a more specific reason
/// would promise operators a signal we cannot deliver.
fn outcome_to_result<T>(outcome: ResolveOutcome<T>) -> Result<T, ResolvedRefsError> {
    match outcome {
        ResolveOutcome::Ok(t) => Ok(t),
        ResolveOutcome::Pending(PendingReason::SecretNotFound { namespace, name }) => {
            Err(ResolvedRefsError::SecretNotFound { namespace, name })
        }
        ResolveOutcome::Pending(PendingReason::ReferenceGrantMissing {
            from_ns,
            to_ns,
            secret_name,
        }) => Err(ResolvedRefsError::RefNotPermitted {
            from_namespace: from_ns,
            from_kind: ResourceKind::LinkSys.as_str(),
            target_namespace: to_ns,
            target_name: secret_name,
        }),
        ResolveOutcome::Invalid(InvalidCredentialError::MissingKeys(keys)) => {
            Err(ResolvedRefsError::MissingKeys { keys })
        }
        ResolveOutcome::Invalid(InvalidCredentialError::DecodeError { key, detail }) => {
            Err(ResolvedRefsError::DecodeError { key, detail })
        }
        ResolveOutcome::Invalid(InvalidCredentialError::InvalidSecretType { got, allowed }) => {
            Err(ResolvedRefsError::InvalidSecretType { got, allowed })
        }
    }
}

/// Build a `ResolvedRefs` condition reflecting the outcome of
/// `resolve_for_spec` and merge it into `ls.status.conditions`.
///
/// Delegates reason/message formatting to `status_utils::resolved_refs_condition`
/// so all credential-backed CRDs produce the same wording. This MUST run even
/// on success so the condition transitions `False → True` when an operator
/// fixes the Secret.
fn apply_resolved_refs_condition(ls: &mut LinkSys, generation: Option<i64>, errors: &[ResolvedRefsError]) {
    let status = ls.status.get_or_insert_with(LinkSysStatus::default);
    update_condition(&mut status.conditions, resolved_refs_condition(errors, generation));
}

/// Does this resolution error require the operator to edit Secret *content*
/// (keys / value format / Secret type) to recover, as opposed to just
/// creating a missing Secret or ReferenceGrant?
///
/// - `false` (info-level log) — the Secret or ReferenceGrant is simply absent
///   or denied. Reprocessing will pick up the fix once the missing resource
///   arrives; no log volume amplification needed.
/// - `true` (warn-level log) — the Secret exists but its content is unusable.
///   Re-queues without operator edits won't help, so louder logging is
///   justified. `RefNotPermitted` is intentionally *not* in this bucket even
///   though "operator forgot Grant" is an edit-to-fix case, because the
///   pipeline cannot distinguish "forgot to create Grant" from "deliberately
///   denied".
fn needs_operator_secret_edit(err: &ResolvedRefsError) -> bool {
    matches!(
        err,
        ResolvedRefsError::MissingKeys { .. }
            | ResolvedRefsError::DecodeError { .. }
            | ResolvedRefsError::InvalidSecretType { .. }
    )
}

#[async_trait::async_trait]
impl ProcessorHandler<LinkSys> for LinkSysHandler {
    fn preparse(&self, ls: &mut LinkSys, _ctx: &HandlerContext) -> Vec<String> {
        ls.validate_config()
    }

    async fn parse(&self, mut ls: LinkSys, ctx: &HandlerContext) -> ProcessResult<LinkSys> {
        let owner = ResourceRef::new(
            ResourceKind::LinkSys,
            ls.metadata.namespace.clone(),
            ls.metadata.name.clone().unwrap_or_default(),
        );

        // Clear old references first (for update scenario).
        // Both managers must be cleared so that changing target type (e.g. Service → Uri)
        // does not leave stale registrations that trigger spurious requeues.
        ctx.secret_ref_manager().clear_resource_refs(&owner);
        ctx.runtime().service_ref_mgr.clear_resource_refs(&owner);

        let errors = resolve_for_spec(ctx, &owner, &mut ls.spec.config).await;
        let generation = ls.metadata.generation;
        apply_resolved_refs_condition(&mut ls, generation, &errors);

        for err in &errors {
            if needs_operator_secret_edit(err) {
                // Invalid Secret content: requeues without Secret edits won't
                // help. The spec is still pushed to the Gateway via conf_sync
                // with `auth.secret` / `tls.certs.secret` left as `None`
                // (partially resolved when only one side failed). Gateway
                // providers must check `CredentialState::is_buildable` and
                // refuse to build a client on `Pending` / `Invalid` material;
                // see `core::gateway::link_sys::credential` for the contract.
                tracing::warn!(
                    link_sys = %owner.key(),
                    ?err,
                    "Credential resolution failed (invalid Secret data)"
                );
            } else {
                tracing::info!(
                    link_sys = %owner.key(),
                    ?err,
                    "Credential resolution pending, will reprocess when Secret/ReferenceGrant arrives"
                );
            }
        }

        // Continue either way so `update_status` persists `ResolvedRefs` and
        // the resource stays alive for Secret/Grant wakeups.
        ProcessResult::Continue(ls)
    }

    async fn on_delete(&self, ls: &LinkSys, ctx: &HandlerContext) {
        let owner = ResourceRef::new(
            ResourceKind::LinkSys,
            ls.metadata.namespace.clone(),
            ls.metadata.name.clone().unwrap_or_default(),
        );
        ctx.secret_ref_manager().clear_resource_refs(&owner);
        ctx.runtime().service_ref_mgr.clear_resource_refs(&owner);
        tracing::debug!(
            link_sys = %owner.key(),
            "Cleared SecretRefManager and ServiceRefManager references on LinkSys delete"
        );
    }

    fn update_status(&self, ls: &mut LinkSys, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = ls.metadata.generation;

        // `ResolvedRefs` is maintained by `parse` (see `apply_resolved_refs_condition`);
        // this method only owns the `Accepted` condition (spec-level validation).
        let status = ls.status.get_or_insert_with(LinkSysStatus::default);

        if validation_errors.is_empty() {
            update_condition(&mut status.conditions, accepted_condition(generation));
        } else {
            update_condition(
                &mut status.conditions,
                condition_false(
                    condition_types::ACCEPTED,
                    "Invalid",
                    validation_errors.join("; "),
                    generation,
                ),
            );
        }
    }
}

// ============================================================================
// Helper functions: resolve Secret per auth variant and inject into spec
// ============================================================================

async fn drive_standard<A: AuthSpec, T: TlsSpec>(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    auth: Option<&mut A>,
    tls: Option<&mut T>,
) -> Vec<ResolvedRefsError> {
    let mut errs = Vec::new();
    if let Some(a) = auth {
        if let Some(e) = a.resolve(ctx, owner).await {
            errs.push(e);
        }
    }
    if let Some(t) = tls {
        if let Some(e) = t.resolve(ctx, owner).await {
            errs.push(e);
        }
    }
    errs
}

/// Attempt to resolve every Secret reference inside `config` and attach the
/// resolved Secret onto the corresponding `auth.secret` / `tls.certs.secret`
/// field. Returns the collected `ResolvedRefsError`s (empty on full success).
///
/// **Multi-error collection**: auth and tls are resolved independently —
/// a failure on one side does not short-circuit the other. This matches the
/// Gateway-API convention where `ResolvedRefs.message` aggregates all broken
/// refs and `reason` is picked deterministically via
/// `pick_resolved_refs_reason`'s priority table. Without this, an operator
/// who mistypes both Secret names only sees the auth error first, then has
/// to reconcile again to discover the tls error — and the priority table
/// effectively never fires because the slice always has one element.
///
/// Leaving `secret: None` on the unresolved field is safe: gateway providers
/// only build a client when `CredentialState::is_buildable` returns true, so
/// a partially resolved spec never reaches a live client.
async fn resolve_for_spec(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    config: &mut SystemConfig,
) -> Vec<ResolvedRefsError> {
    match config {
        SystemConfig::Redis(r) => drive_standard(ctx, owner, r.auth.as_mut(), r.tls.as_mut()).await,
        SystemConfig::Etcd(e) => drive_standard(ctx, owner, e.auth.as_mut(), e.tls.as_mut()).await,
        SystemConfig::Elasticsearch(es) => drive_standard(ctx, owner, es.auth.as_mut(), es.tls.as_mut()).await,
        SystemConfig::Webhook(w) => {
            let errors = drive_standard(ctx, owner, w.auth.as_mut(), w.tls.as_mut()).await;

            if let WebhookTarget::Service {
                name,
                namespace,
                port,
                kind,
                endpoints,
                ..
            } = &mut w.target
            {
                if matches!(
                    EdgionService::from_kind(kind.as_ref()),
                    EdgionService::Service | EdgionService::ServiceEndpointSlice
                ) {
                    let ns = namespace.as_deref().or(owner.namespace.as_deref()).unwrap_or("default");
                    let svc_key = format!("{ns}/{name}");

                    ctx.runtime().service_ref_mgr.add_ref(svc_key.clone(), owner.clone());

                    *endpoints = resolve_endpoints(ctx, ns, name, *port);

                    if endpoints.is_empty() {
                        tracing::debug!(
                            link_sys = %owner.key(),
                            service = %svc_key,
                            "Webhook service endpoints not yet available (EndpointSlice pending)"
                        );
                    }
                }
            }
            errors
        }
        SystemConfig::Kafka(_) => Vec::new(),
    }
}

/// Collect ready pod IP:port strings for a Service from cached EndpointSlice objects.
fn resolve_endpoints(ctx: &HandlerContext, namespace: &str, svc_name: &str, port: Option<i32>) -> Vec<String> {
    let Some(proc) = ctx.runtime().processor_registry.get("EndpointSlice") else {
        return vec![];
    };
    proc.list_keys()
        .into_iter()
        .filter_map(|key| {
            let any = proc.get_raw_any(&key)?;
            any.downcast::<EndpointSlice>().ok()
        })
        .filter(|eps| {
            eps.metadata.namespace.as_deref() == Some(namespace)
                && eps
                    .metadata
                    .labels
                    .as_ref()
                    .and_then(|l| l.get("kubernetes.io/service-name"))
                    .map(|s| s == svc_name)
                    .unwrap_or(false)
        })
        .flat_map(|eps| extract_ready_ips(&eps, port))
        .collect()
}

/// Extract `"ip:port"` strings for ready endpoints from a single EndpointSlice.
fn extract_ready_ips(eps: &EndpointSlice, port: Option<i32>) -> Vec<String> {
    let target_port = port;
    let ports = eps.ports.as_deref().unwrap_or(&[]);
    if target_port.is_none() && ports.len() > 1 {
        tracing::debug!(
            eps = eps.metadata.name.as_deref().unwrap_or(""),
            port_count = ports.len(),
            "Webhook service target has no port specified; selecting first available EndpointSlice port"
        );
    }
    let ep_port = ports
        .iter()
        .find(|p| target_port.is_none() || p.port == target_port)
        .and_then(|p| p.port);
    let Some(ep_port) = ep_port else {
        return vec![];
    };

    eps.endpoints
        .iter()
        .filter(|ep| ep.conditions.as_ref().and_then(|c| c.ready).unwrap_or(false))
        .flat_map(|ep| ep.addresses.iter().map(move |ip| format!("{ip}:{ep_port}")))
        .collect()
}

pub(super) async fn resolve_one<R: SecretMaterialResolver>(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    secret_ref: &SecretObjectReference,
    resolver: &R,
) -> Result<Option<Secret>, ResolvedRefsError> {
    let outcome = resolve_secret(ctx, owner, "edgion.io", "LinkSys", secret_ref, resolver).await;
    outcome_to_result(outcome).map(Some)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::controller::conf_mgr::sync_runtime::resource_processor::condition_reasons;
    use crate::core::controller::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::core::controller::conf_mgr::ControllerRuntime;
    use crate::types::credentials::resolvers::{BasicAuthResolver, EsApiKeyResolver};
    use crate::types::resources::common::{Condition, SecretSlot};
    use crate::types::resources::link_sys::elasticsearch::ElasticsearchAuth;
    use crate::types::resources::link_sys::redis::{RedisAuth, RedisClientConfig};
    use crate::types::resources::link_sys::LinkSysSpec;
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use kube::api::ObjectMeta;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;

    fn find_cond<'a>(ls: &'a LinkSys, type_: &str) -> Option<&'a Condition> {
        ls.status.as_ref()?.conditions.iter().find(|c| c.type_ == type_)
    }

    fn make_ctx(secrets: HashMap<String, Secret>) -> (HandlerContext, Arc<ControllerRuntime>) {
        let runtime = Arc::new(ControllerRuntime::default());
        runtime.secret_store.replace_all(secrets);
        let ctx = HandlerContext::new(
            None,
            None,
            TriggerChain::default(),
            5,
            20,
            runtime.clone(),
        );
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
        let key = format!("{}/{}", namespace, name);
        (key, secret)
    }

    fn make_redis_link_sys(ns: &str, name: &str, secret_name: &str, secret_ns: Option<&str>) -> LinkSys {
        LinkSys {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(ns.to_string()),
                generation: Some(1),
                ..Default::default()
            },
            spec: LinkSysSpec {
                config: SystemConfig::Redis(RedisClientConfig {
                    endpoints: vec!["redis://localhost:6379".to_string()],
                    auth: Some(RedisAuth {
                        slot: crate::types::resources::common::SecretSlot {
                            secret_ref: SecretObjectReference {
                                group: None,
                                kind: None,
                                name: secret_name.to_string(),
                                namespace: secret_ns.map(|s| s.to_string()),
                            },
                            secret: None,
                        },
                    }),
                    db: None,
                    timeout: None,
                    pool: None,
                    retry: None,
                    topology: None,
                    tls: None,
                    observability: None,
                }),
            },
            status: None,
        }
    }

    fn make_es_link_sys_apikey(ns: &str, name: &str, secret_name: &str) -> LinkSys {
        use crate::types::resources::link_sys::elasticsearch::ElasticsearchClientConfig;
        LinkSys {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(ns.to_string()),
                generation: Some(1),
                ..Default::default()
            },
            spec: LinkSysSpec {
                config: SystemConfig::Elasticsearch(ElasticsearchClientConfig {
                    endpoints: vec!["http://localhost:9200".to_string()],
                    auth: Some(ElasticsearchAuth::ApiKey {
                        slot: SecretSlot {
                            secret_ref: SecretObjectReference {
                                group: None,
                                kind: None,
                                name: secret_name.to_string(),
                                namespace: None,
                            },
                            secret: None,
                        },
                    }),
                    tls: None,
                    timeout: None,
                    pool: None,
                    bulk: None,
                    index: None,
                    failed_cache: None,
                    compatibility: None,
                }),
            },
            status: None,
        }
    }

    #[tokio::test]
    async fn test_redis_auth_happy_path() {
        let (key, secret) = make_secret(
            "default",
            "redis-creds",
            &[("username", b"admin"), ("password", b"s3cr3t")],
        );
        let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
        let handler = LinkSysHandler::new();
        let ls = make_redis_link_sys("default", "my-redis", "redis-creds", None);

        let result = handler.parse(ls, &ctx).await;

        match result {
            ProcessResult::Continue(ls) => {
                let auth = match &ls.spec.config {
                    SystemConfig::Redis(r) => r.auth.as_ref().expect("auth should be present"),
                    _ => panic!("expected Redis"),
                };
                let secret = auth.slot.secret.as_ref().expect("secret should be attached");
                // parse material from embedded secret via resolver
                let data = secret.data.as_ref().expect("secret data");
                let m = BasicAuthResolver.parse(data).expect("parse material");
                assert_eq!(m.username.as_deref(), Some("admin"));
                assert_eq!(m.password, "s3cr3t");
                let rr = find_cond(&ls, condition_types::RESOLVED_REFS).expect("ResolvedRefs");
                assert_eq!(rr.status, "True");
                assert_eq!(rr.reason, condition_reasons::RESOLVED_REFS);
            }
            ProcessResult::Skip { reason } => panic!("expected Continue, got Skip: {reason}"),
        }
    }

    #[tokio::test]
    async fn test_secret_not_found_returns_pending() {
        let (ctx, _runtime) = make_ctx(HashMap::new());
        let handler = LinkSysHandler::new();
        let ls = make_redis_link_sys("default", "my-redis", "missing-secret", None);

        let result = handler.parse(ls, &ctx).await;

        match result {
            ProcessResult::Continue(ls) => {
                let auth = match &ls.spec.config {
                    SystemConfig::Redis(r) => r.auth.as_ref().expect("auth should be present"),
                    _ => panic!("expected Redis"),
                };
                assert!(auth.slot.secret.is_none(), "secret should be None while pending");
                let refs = ctx.secret_ref_manager().get_refs("default/missing-secret");
                let owner = ResourceRef::new(
                    ResourceKind::LinkSys,
                    Some("default".to_string()),
                    "my-redis".to_string(),
                );
                assert!(refs.contains(&owner), "SecretRefManager should contain owner");
                let rr = find_cond(&ls, condition_types::RESOLVED_REFS).expect("ResolvedRefs");
                assert_eq!(rr.status, "False");
                assert_eq!(rr.reason, condition_reasons::SECRET_NOT_FOUND);
            }
            ProcessResult::Skip { reason } => panic!("expected Continue (pending), got Skip: {reason}"),
        }
    }

    #[tokio::test]
    async fn test_es_apikey_auth() {
        let (key, secret) = make_secret(
            "default",
            "es-apikey",
            &[("apiKeyId", b"abc123"), ("apiKeySecret", b"secretXYZ")],
        );
        let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
        let handler = LinkSysHandler::new();
        let ls = make_es_link_sys_apikey("default", "my-es", "es-apikey");

        let result = handler.parse(ls, &ctx).await;

        match result {
            ProcessResult::Continue(ls) => {
                let auth = match &ls.spec.config {
                    SystemConfig::Elasticsearch(es) => es.auth.as_ref().expect("auth"),
                    _ => panic!("expected Elasticsearch"),
                };
                let ElasticsearchAuth::ApiKey { slot } = auth else {
                    panic!("expected ApiKey variant");
                };
                let data = slot.secret.as_ref().expect("secret").data.as_ref().expect("data");
                let m = EsApiKeyResolver.parse(data).expect("parse");
                assert_eq!(m.api_key_id, "abc123");
                assert_eq!(m.api_key_secret, "secretXYZ");
                let rr = find_cond(&ls, condition_types::RESOLVED_REFS).expect("ResolvedRefs");
                assert_eq!(rr.status, "True");
            }
            ProcessResult::Skip { reason } => panic!("expected Continue, got Skip: {reason}"),
        }
    }

    #[tokio::test]
    async fn test_on_delete_clears_secret_ref_manager() {
        let (key, secret) = make_secret("default", "redis-creds", &[("password", b"s3cr3t")]);
        let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
        let handler = LinkSysHandler::new();
        let ls = make_redis_link_sys("default", "my-redis", "redis-creds", None);

        let result = handler.parse(ls.clone(), &ctx).await;
        assert!(matches!(result, ProcessResult::Continue(_)));

        let refs = ctx.secret_ref_manager().get_refs("default/redis-creds");
        let owner = ResourceRef::new(
            ResourceKind::LinkSys,
            Some("default".to_string()),
            "my-redis".to_string(),
        );
        assert!(refs.contains(&owner), "reference should be registered after parse");

        handler.on_delete(&ls, &ctx).await;

        let refs = ctx.secret_ref_manager().get_refs("default/redis-creds");
        assert!(!refs.contains(&owner), "reference should be cleared after on_delete");
    }

    #[tokio::test]
    async fn test_cross_ns_without_grant_sets_ref_not_permitted() {
        let (key, secret) = make_secret("shared", "redis-creds", &[("password", b"s3cr3t")]);
        let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
        let handler = LinkSysHandler::new();
        // Owner is in `app`, Secret is in `shared` — no ReferenceGrant configured.
        let ls = make_redis_link_sys("app", "my-redis", "redis-creds", Some("shared"));

        let result = handler.parse(ls, &ctx).await;

        match result {
            ProcessResult::Continue(ls) => {
                let auth = match &ls.spec.config {
                    SystemConfig::Redis(r) => r.auth.as_ref().expect("auth should be present"),
                    _ => panic!("expected Redis"),
                };
                assert!(
                    auth.slot.secret.is_none(),
                    "secret must not be embedded while cross-ns reference is not permitted"
                );
                let rr = find_cond(&ls, condition_types::RESOLVED_REFS).expect("ResolvedRefs");
                assert_eq!(rr.status, "False");
                // Cross-ns denial collapses onto the Gateway-API standard
                // `RefNotPermitted` reason — the pipeline cannot distinguish
                // "no grant yet" from "grant denies", and the upstream spec
                // explicitly uses a single reason for both.
                assert_eq!(rr.reason, condition_reasons::REF_NOT_PERMITTED);
                // Message must still name both endpoints so operators can
                // diagnose which grant to create.
                assert!(
                    rr.message.contains("shared/redis-creds"),
                    "message must identify target 'shared/redis-creds', got: {}",
                    rr.message
                );
                assert!(
                    rr.message.contains("ReferenceGrant"),
                    "message must name the missing mechanism, got: {}",
                    rr.message
                );
                // Even while denied, the owner must be registered so a later
                // ReferenceGrant / Secret event can wake this resource.
                let refs = ctx.secret_ref_manager().get_refs("shared/redis-creds");
                let owner = ResourceRef::new(ResourceKind::LinkSys, Some("app".to_string()), "my-redis".to_string());
                assert!(refs.contains(&owner), "SecretRefManager should register cross-ns owner");
            }
            ProcessResult::Skip { reason } => panic!("expected Continue (pending), got Skip: {reason}"),
        }
    }

    /// After a successful parse leaves `ResolvedRefs=True`, a subsequent parse
    /// against a broken Secret must flip the condition to `False` with the
    /// correct reason — not silently retain `True`.
    ///
    /// The real flow re-parses on Secret changes (wakeup via SecretRefManager);
    /// here we simulate it by swapping the Secret in the store between two
    /// `parse` calls and re-using the status from the prior pass.
    #[tokio::test]
    async fn test_resolved_refs_transitions_true_to_false_when_secret_broken() {
        let (key, good) = make_secret("default", "redis-creds", &[("password", b"s3cr3t")]);
        let (ctx, runtime) = make_ctx(HashMap::from([(key.clone(), good)]));
        let handler = LinkSysHandler::new();
        let ls = make_redis_link_sys("default", "my-redis", "redis-creds", None);

        let ls = match handler.parse(ls, &ctx).await {
            ProcessResult::Continue(ls) => ls,
            ProcessResult::Skip { reason } => panic!("expected Continue, got Skip: {reason}"),
        };
        let rr = find_cond(&ls, condition_types::RESOLVED_REFS).expect("ResolvedRefs v1");
        assert_eq!(rr.status, "True", "initial parse must mark ResolvedRefs=True");

        // Carry the prior status forward onto a fresh spec (mirrors what the
        // informer delivers: spec without `auth.secret`, but status preserved
        // in-cluster from the previous reconcile).
        let mut next = make_redis_link_sys("default", "my-redis", "redis-creds", None);
        next.status = ls.status.clone();

        // Operator drops the required `password` key in the Secret.
        let (_, broken) = make_secret("default", "redis-creds", &[("wrong_key", b"oops")]);
        runtime.secret_store.replace_all(HashMap::from([(key, broken)]));

        let next = match handler.parse(next, &ctx).await {
            ProcessResult::Continue(ls) => ls,
            ProcessResult::Skip { reason } => panic!("expected Continue, got Skip: {reason}"),
        };
        let rr = find_cond(&next, condition_types::RESOLVED_REFS).expect("ResolvedRefs v2");
        assert_eq!(rr.status, "False", "broken Secret must flip ResolvedRefs to False");
        assert_eq!(rr.reason, condition_reasons::MISSING_KEYS);
        // `update_condition` merges by type — the prior True condition must be
        // replaced in place, not duplicated.
        let rr_count = next
            .status
            .as_ref()
            .expect("status")
            .conditions
            .iter()
            .filter(|c| c.type_ == condition_types::RESOLVED_REFS)
            .count();
        assert_eq!(rr_count, 1, "conditions must be merged by type, not duplicated");
    }

    #[tokio::test]
    async fn test_invalid_secret_data_sets_resolved_refs_false_and_continues() {
        let (key, secret) = make_secret("default", "redis-creds", &[("wrong_key", b"some_value")]);
        let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
        let handler = LinkSysHandler::new();
        let ls = make_redis_link_sys("default", "my-redis", "redis-creds", None);

        let result = handler.parse(ls, &ctx).await;

        match result {
            ProcessResult::Continue(ls) => {
                let rr = find_cond(&ls, condition_types::RESOLVED_REFS).expect("ResolvedRefs");
                assert_eq!(rr.status, "False");
                assert_eq!(rr.reason, condition_reasons::MISSING_KEYS);
                assert!(rr.message.contains("password"), "message should mention missing key");
                // Invalid data must NOT leave a stale Secret embedded on spec
                // — defence-in-depth so Gateway providers (which already gate
                // on `CredentialState::is_buildable`) never see bad material
                // even if a future change widens dispatch.
                let auth = match &ls.spec.config {
                    SystemConfig::Redis(r) => r.auth.as_ref().expect("auth"),
                    _ => panic!("expected Redis"),
                };
                assert!(auth.slot.secret.is_none(), "secret must not be attached on Invalid");
            }
            ProcessResult::Skip { reason } => panic!("expected Continue so status persists, got Skip: {reason}"),
        }
    }

    /// `DecodeError` variant: the Secret has the required key but the
    /// value is not UTF-8, so `BasicAuthResolver::parse` surfaces
    /// `ResolveError::DecodeError`, which pipeline maps to
    /// `InvalidCredentialError::DecodeError`.
    ///
    /// We assert that:
    /// - `parse` returns `Continue` (not `Skip`) so status gets persisted
    /// - `ResolvedRefs=False` with reason `DecodeError`
    /// - The condition message pinpoints the offending key
    /// - No stale Secret is attached to spec
    #[tokio::test]
    async fn test_decode_error_sets_resolved_refs_false_with_decode_error_reason() {
        // Invalid UTF-8 in `password` — resolver rejects before returning material.
        let (key, secret) = make_secret("default", "redis-creds", &[("password", &[0xff, 0xfe, 0xfd][..])]);
        let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
        let handler = LinkSysHandler::new();
        let ls = make_redis_link_sys("default", "my-redis", "redis-creds", None);

        let result = handler.parse(ls, &ctx).await;

        match result {
            ProcessResult::Continue(ls) => {
                let rr = find_cond(&ls, condition_types::RESOLVED_REFS).expect("ResolvedRefs");
                assert_eq!(rr.status, "False");
                assert_eq!(rr.reason, condition_reasons::DECODE_ERROR);
                assert!(
                    rr.message.contains("password"),
                    "DecodeError message must name the offending key 'password', got: {}",
                    rr.message
                );
                let auth = match &ls.spec.config {
                    SystemConfig::Redis(r) => r.auth.as_ref().expect("auth"),
                    _ => panic!("expected Redis"),
                };
                assert!(auth.slot.secret.is_none(), "secret must not be attached on DecodeError");
            }
            ProcessResult::Skip { reason } => panic!("expected Continue so status persists, got Skip: {reason}"),
        }
    }

    /// `InvalidSecretType` variant: the Secret data is fine, but its
    /// `type` is not in `BasicAuthResolver::allowed_secret_types()`
    /// (`kubernetes.io/basic-auth`, `Opaque`, or missing).
    ///
    /// This check runs in `credential/pipeline.rs::check_secret_type` before
    /// `resolver.parse`, so we verify that the handler surfaces it with the
    /// correct reason and a message that mentions the rejected type.
    #[tokio::test]
    async fn test_invalid_secret_type_sets_resolved_refs_false_with_invalid_secret_type_reason() {
        let (key, mut secret) = make_secret("default", "redis-creds", &[("password", b"s3cr3t")]);
        // `kubernetes.io/ssh-auth` is a real Secret type but not accepted here.
        secret.type_ = Some("kubernetes.io/ssh-auth".to_string());
        let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
        let handler = LinkSysHandler::new();
        let ls = make_redis_link_sys("default", "my-redis", "redis-creds", None);

        let result = handler.parse(ls, &ctx).await;

        match result {
            ProcessResult::Continue(ls) => {
                let rr = find_cond(&ls, condition_types::RESOLVED_REFS).expect("ResolvedRefs");
                assert_eq!(rr.status, "False");
                assert_eq!(rr.reason, condition_reasons::INVALID_SECRET_TYPE);
                assert!(
                    rr.message.contains("ssh-auth"),
                    "message must identify the rejected type, got: {}",
                    rr.message
                );
                let auth = match &ls.spec.config {
                    SystemConfig::Redis(r) => r.auth.as_ref().expect("auth"),
                    _ => panic!("expected Redis"),
                };
                assert!(
                    auth.slot.secret.is_none(),
                    "secret must not be attached on InvalidSecretType"
                );
            }
            ProcessResult::Skip { reason } => panic!("expected Continue so status persists, got Skip: {reason}"),
        }
    }

    #[tokio::test]
    async fn test_no_auth_configured() {
        let (ctx, _runtime) = make_ctx(HashMap::new());
        let handler = LinkSysHandler::new();

        let ls = LinkSys {
            metadata: ObjectMeta {
                name: Some("my-redis".to_string()),
                namespace: Some("default".to_string()),
                generation: Some(1),
                ..Default::default()
            },
            spec: LinkSysSpec {
                config: SystemConfig::Redis(RedisClientConfig {
                    endpoints: vec!["redis://localhost:6379".to_string()],
                    auth: None,
                    db: None,
                    timeout: None,
                    pool: None,
                    retry: None,
                    topology: None,
                    tls: None,
                    observability: None,
                }),
            },
            status: None,
        };

        let result = handler.parse(ls, &ctx).await;

        match result {
            ProcessResult::Continue(ls) => {
                let auth = match &ls.spec.config {
                    SystemConfig::Redis(r) => r.auth.as_ref(),
                    _ => panic!("expected Redis"),
                };
                assert!(auth.is_none(), "auth should be None when not configured");
                let rr = find_cond(&ls, condition_types::RESOLVED_REFS).expect("ResolvedRefs");
                assert_eq!(rr.status, "True");
            }
            ProcessResult::Skip { reason } => panic!("expected Continue, got Skip: {reason}"),
        }
    }

    #[test]
    fn test_update_status_sets_accepted_true() {
        let (ctx, _runtime) = make_ctx(HashMap::new());
        let handler = LinkSysHandler::new();
        let mut ls = make_redis_link_sys("default", "my-redis", "redis-creds", None);

        handler.update_status(&mut ls, &ctx, &[]);

        let status = ls.status.expect("status should be set");
        let accepted = status
            .conditions
            .iter()
            .find(|c| c.type_ == "Accepted")
            .expect("Accepted condition should exist");
        assert_eq!(accepted.status, "True");
        assert_eq!(accepted.reason, "Accepted");
        assert!(
            status
                .conditions
                .iter()
                .all(|c| c.type_ != condition_types::RESOLVED_REFS),
            "update_status must not touch ResolvedRefs (set in parse)"
        );
    }

    #[test]
    fn test_update_status_sets_accepted_false_on_validation_errors() {
        let (ctx, _runtime) = make_ctx(HashMap::new());
        let handler = LinkSysHandler::new();
        let mut ls = make_redis_link_sys("default", "my-redis", "redis-creds", None);

        let errors = vec!["Redis configuration has no endpoints".to_string()];
        handler.update_status(&mut ls, &ctx, &errors);

        let status = ls.status.expect("status should be set");
        let accepted = status
            .conditions
            .iter()
            .find(|c| c.type_ == "Accepted")
            .expect("Accepted condition should exist");
        assert_eq!(accepted.status, "False");
        assert_eq!(accepted.reason, "Invalid");
        assert!(accepted.message.contains("no endpoints"));
    }

    #[tokio::test]
    async fn test_redis_tls_secret_ref_resolution() {
        use crate::types::credentials::resolvers::{TLS_CA_KEY, TLS_CERT_KEY, TLS_KEY_KEY};
        use crate::types::resources::common::{TlsCerts, TlsConfig};

        let (key, mut secret) = make_secret(
            "default",
            "redis-tls",
            &[
                (TLS_CA_KEY, b"CA-PEM"),
                (TLS_CERT_KEY, b"CERT-PEM"),
                (TLS_KEY_KEY, b"KEY-PEM"),
            ],
        );
        secret.type_ = Some("kubernetes.io/tls".to_string());

        let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
        let handler = LinkSysHandler::new();

        let mut ls = make_redis_link_sys("default", "my-redis", "redis-creds", None);
        ls.spec.config = SystemConfig::Redis(RedisClientConfig {
            endpoints: vec!["rediss://localhost:6380".to_string()],
            auth: None,
            db: None,
            timeout: None,
            pool: None,
            retry: None,
            topology: None,
            tls: Some(TlsConfig {
                enabled: true,
                certs: Some(TlsCerts {
                    ca_cert: None,
                    client_cert: None,
                    secret_ref: Some(SecretObjectReference {
                        group: None,
                        kind: None,
                        name: "redis-tls".to_string(),
                        namespace: None,
                    }),
                    secret: None,
                }),
                insecure_skip_verify: None,
            }),
            observability: None,
        });

        let ls = match handler.parse(ls, &ctx).await {
            ProcessResult::Continue(ls) => ls,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };

        let certs = match &ls.spec.config {
            SystemConfig::Redis(r) => r.tls.as_ref().unwrap().certs.as_ref().unwrap(),
            _ => panic!("expected Redis"),
        };
        let secret = certs.secret.as_ref().expect("secret injected");
        let data = secret.data.as_ref().expect("data");
        assert_eq!(data.get(TLS_CA_KEY).unwrap().0, b"CA-PEM");
        assert_eq!(data.get(TLS_CERT_KEY).unwrap().0, b"CERT-PEM");
        assert_eq!(data.get(TLS_KEY_KEY).unwrap().0, b"KEY-PEM");
        let rr = find_cond(&ls, condition_types::RESOLVED_REFS).expect("ResolvedRefs");
        assert_eq!(rr.status, "True");
    }

    /// Multi-error regression: `resolve_for_spec` must NOT short-circuit on
    /// the first auth failure — tls must also be resolved so both errors land
    /// in the same `ResolvedRefs` condition.
    ///
    /// Setup:
    /// - auth.secretRef points to a non-existent Secret → `SecretNotFound`
    ///   (priority 5 in `pick_resolved_refs_reason`)
    /// - tls.certs.secretRef points to a Secret with a rejected `type`
    ///   (`kubernetes.io/ssh-auth`, which `TlsCertsResolver` does not allow)
    ///   → `InvalidSecretType` (priority 2 — higher severity)
    ///
    /// Assertions:
    /// - `errors.len() == 2` (proves we walked both fields)
    /// - `reason == InvalidSecretType` (proves priority table fires)
    /// - `message` joins both error texts so operators see both problems
    #[tokio::test]
    async fn test_auth_and_tls_both_fail_collects_both_errors() {
        use crate::types::credentials::resolvers::{TLS_CA_KEY, TLS_CERT_KEY, TLS_KEY_KEY};
        use crate::types::resources::common::{TlsCerts, TlsConfig};

        // tls Secret exists but with a rejected type — surfaces as
        // `InvalidSecretType` from the pipeline's type-check step.
        let (tls_key, mut tls_secret) = make_secret(
            "default",
            "redis-tls-bad-type",
            &[
                (TLS_CA_KEY, b"CA-PEM"),
                (TLS_CERT_KEY, b"CERT-PEM"),
                (TLS_KEY_KEY, b"KEY-PEM"),
            ],
        );
        tls_secret.type_ = Some("kubernetes.io/ssh-auth".to_string());

        let (ctx, _runtime) = make_ctx(HashMap::from([(tls_key, tls_secret)]));
        let handler = LinkSysHandler::new();

        // auth points to a Secret that doesn't exist in the store.
        let mut ls = make_redis_link_sys("default", "my-redis", "missing-auth", None);
        ls.spec.config = SystemConfig::Redis(RedisClientConfig {
            endpoints: vec!["rediss://localhost:6380".to_string()],
            auth: Some(RedisAuth {
                slot: crate::types::resources::common::SecretSlot {
                    secret_ref: SecretObjectReference {
                        group: None,
                        kind: None,
                        name: "missing-auth".to_string(),
                        namespace: None,
                    },
                    secret: None,
                },
            }),
            db: None,
            timeout: None,
            pool: None,
            retry: None,
            topology: None,
            tls: Some(TlsConfig {
                enabled: true,
                certs: Some(TlsCerts {
                    ca_cert: None,
                    client_cert: None,
                    secret_ref: Some(SecretObjectReference {
                        group: None,
                        kind: None,
                        name: "redis-tls-bad-type".to_string(),
                        namespace: None,
                    }),
                    secret: None,
                }),
                insecure_skip_verify: None,
            }),
            observability: None,
        });

        let ls = match handler.parse(ls, &ctx).await {
            ProcessResult::Continue(ls) => ls,
            ProcessResult::Skip { reason } => panic!("expected Continue, got Skip: {reason}"),
        };

        // Spec must show both sides still at `secret: None`, proving neither
        // side silently succeeded.
        match &ls.spec.config {
            SystemConfig::Redis(r) => {
                assert!(
                    r.auth.as_ref().unwrap().slot.secret.is_none(),
                    "auth.secret must remain None when auth resolution failed"
                );
                assert!(
                    r.tls.as_ref().unwrap().certs.as_ref().unwrap().secret.is_none(),
                    "tls.certs.secret must remain None when tls resolution failed"
                );
            }
            _ => panic!("expected Redis"),
        }

        let rr = find_cond(&ls, condition_types::RESOLVED_REFS).expect("ResolvedRefs");
        assert_eq!(rr.status, "False");
        // `InvalidSecretType` (priority 2) wins over `SecretNotFound`
        // (priority 5) per `pick_resolved_refs_reason`. If this flips back
        // to `SecretNotFound`, it means the Redis branch short-circuited on
        // auth again — multi-error aggregation regression.
        assert_eq!(
            rr.reason,
            condition_reasons::INVALID_SECRET_TYPE,
            "picked reason must be the highest-priority error (InvalidSecretType), got: {}",
            rr.reason,
        );
        // Both error messages must appear — `resolved_refs_condition` joins
        // them with "; ". Checking for tokens unique to each error proves
        // the join is actually happening.
        assert!(
            rr.message.contains("missing-auth"),
            "message must mention the missing auth Secret, got: {}",
            rr.message,
        );
        assert!(
            rr.message.contains("ssh-auth"),
            "message must mention the rejected tls Secret type, got: {}",
            rr.message,
        );
    }

    /// Verify that LinkSys.spec survives a JSON round-trip and preserves the
    /// embedded Secret (mimics conf_sync over gRPC).
    #[tokio::test]
    async fn test_spec_serde_round_trip_preserves_secret() {
        let (key, secret) = make_secret("default", "redis-creds", &[("password", b"s3cr3t")]);
        let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
        let handler = LinkSysHandler::new();
        let ls = make_redis_link_sys("default", "my-redis", "redis-creds", None);

        let ls = match handler.parse(ls, &ctx).await {
            ProcessResult::Continue(ls) => ls,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };

        let json = serde_json::to_string(&ls.spec).expect("serialize spec");
        let parsed: LinkSysSpec = serde_json::from_str(&json).expect("deserialize spec");
        let auth = match &parsed.config {
            SystemConfig::Redis(r) => r.auth.as_ref().expect("auth"),
            _ => panic!("expected Redis"),
        };
        let data = auth.slot.secret.as_ref().expect("secret").data.as_ref().expect("data");
        let m = BasicAuthResolver.parse(data).expect("parse material");
        assert_eq!(m.password, "s3cr3t");
        let rr = find_cond(&ls, condition_types::RESOLVED_REFS).expect("ResolvedRefs");
        assert_eq!(rr.status, "True");
    }

    fn make_webhook_link_sys(ns: &str, name: &str, svc_name: &str, svc_ns: Option<&str>) -> LinkSys {
        use crate::types::resources::link_sys::webhook::{WebhookServiceConfig, WebhookTarget};
        LinkSys {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(ns.to_string()),
                generation: Some(1),
                ..Default::default()
            },
            spec: LinkSysSpec {
                config: SystemConfig::Webhook(WebhookServiceConfig {
                    target: WebhookTarget::Service {
                        name: svc_name.to_string(),
                        namespace: svc_ns.map(str::to_string),
                        port: Some(8080),
                        group: None,
                        kind: None,
                        endpoints: vec![],
                    },
                    ..Default::default()
                }),
            },
            status: None,
        }
    }

    #[tokio::test]
    async fn test_webhook_service_target_registers_service_ref() {
        let (ctx, runtime) = make_ctx(HashMap::new());
        let handler = LinkSysHandler::new();
        let ls = make_webhook_link_sys("default", "my-wh", "auth-svc", None);

        let result = handler.parse(ls, &ctx).await;
        assert!(matches!(result, ProcessResult::Continue(_)));

        // "default/auth-svc" should now be registered in service_ref_mgr.
        let refs = runtime.service_ref_mgr.get_refs("default/auth-svc");
        let owner = ResourceRef::new(ResourceKind::LinkSys, Some("default".to_string()), "my-wh".to_string());
        assert!(refs.contains(&owner), "service_ref_mgr must contain owner after parse");
    }

    #[tokio::test]
    async fn test_webhook_service_target_clears_stale_ref_on_target_change() {
        let (ctx, runtime) = make_ctx(HashMap::new());
        let handler = LinkSysHandler::new();

        // First parse: Service target.
        let ls_svc = make_webhook_link_sys("default", "my-wh", "auth-svc", None);
        handler.parse(ls_svc, &ctx).await;

        let refs = runtime.service_ref_mgr.get_refs("default/auth-svc");
        let owner = ResourceRef::new(ResourceKind::LinkSys, Some("default".to_string()), "my-wh".to_string());
        assert!(refs.contains(&owner), "should be registered after first parse");

        // Second parse: Uri target (user switched webhook type).
        let ls_uri = LinkSys {
            metadata: ObjectMeta {
                name: Some("my-wh".to_string()),
                namespace: Some("default".to_string()),
                generation: Some(2),
                ..Default::default()
            },
            spec: LinkSysSpec {
                config: SystemConfig::Webhook(crate::types::resources::link_sys::webhook::WebhookServiceConfig {
                    target: crate::types::resources::link_sys::webhook::WebhookTarget::Uri {
                        uri: "http://external.example.com/hook".to_string(),
                    },
                    ..Default::default()
                }),
            },
            status: None,
        };
        handler.parse(ls_uri, &ctx).await;

        // Stale registration must be gone after switching to Uri target.
        let refs = runtime.service_ref_mgr.get_refs("default/auth-svc");
        assert!(
            !refs.contains(&owner),
            "stale service_ref_mgr registration must be cleared when target changes to Uri"
        );
    }

    #[tokio::test]
    async fn test_on_delete_clears_service_ref_manager() {
        let (ctx, runtime) = make_ctx(HashMap::new());
        let handler = LinkSysHandler::new();
        let ls = make_webhook_link_sys("default", "my-wh", "auth-svc", None);

        handler.parse(ls.clone(), &ctx).await;

        let owner = ResourceRef::new(ResourceKind::LinkSys, Some("default".to_string()), "my-wh".to_string());
        assert!(
            runtime.service_ref_mgr.get_refs("default/auth-svc").contains(&owner),
            "should be registered before delete"
        );

        handler.on_delete(&ls, &ctx).await;

        assert!(
            !runtime.service_ref_mgr.get_refs("default/auth-svc").contains(&owner),
            "service_ref_mgr should be cleared on delete"
        );
    }
}
