//! Lock-free runtime index for [`EdgionBackend`] resources.
//!
//! The conf-sync path validates and compiles resources into a single immutable
//! snapshot. Request processing captures one ArcSwap generation and performs
//! bounded hash lookups within it; it never reads the typed ConfigClient cache
//! or reparses endpoint, model, or credential data.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, LazyLock};

use arc_swap::ArcSwap;
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::credentials::{AiApiKeyResolver, BearerResolver, SecretMaterialResolver};
use edgion_resources::resources::edgion_backend::{
    AiCredentialLimits, AiProvider, EdgionBackend, McpTransportType, ParsedAiModelPricing, ResolvedA2aTarget,
    ResolvedMcpTarget,
};
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
use edgion_resources::resources::edgion_plugins::OnRedisFailure;
use http::HeaderValue;
use url::{Host, Url};

type RuntimeMap = HashMap<String, Arc<RuntimeEdgionBackend>>;

static GLOBAL_EDGION_BACKEND_STORE: LazyLock<Arc<EdgionBackendStore>> = LazyLock::new(|| {
    Arc::new(EdgionBackendStore::with_credential_outlier_store(
        super::get_ai_credential_outlier_store(),
    ))
});

/// Cold-compiled provider endpoint used by the request-time external jump.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeAiEndpoint {
    /// DNS name or unbracketed IP literal passed to the shared resolver.
    pub domain: String,
    pub port: u16,
    pub use_tls: bool,
    /// SNI value for TLS peer construction.
    pub sni: String,
    /// HTTP Host authority, including a non-default port when configured.
    pub host: String,
    /// Empty for a root endpoint; otherwise begins with `/` and has no trailing `/`.
    pub path_prefix: String,
}

/// Cold-compiled external HTTP endpoint. Resolution still happens through the
/// canonical routed-external resolver at dial time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeExternalHttpEndpoint {
    pub domain: String,
    pub port: u16,
    pub use_tls: bool,
    pub sni: String,
    pub authority: String,
    pub path_prefix: String,
}

/// Controller-proved, request-ready A2A transport target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeA2aTarget {
    Service { namespace: String, name: String, port: u16 },
    External(RuntimeExternalHttpEndpoint),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeA2aBackend {
    /// Stable logical-backend identity for policy and attempt state.
    pub uid: String,
    pub target: RuntimeA2aTarget,
}

/// Fixed MCP protocol/transport identity compiled by WP9.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeMcpTransport {
    StreamableHttp20260728,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeMcpTarget {
    Service {
        namespace: String,
        name: String,
        port: u16,
        path: String,
    },
    External(RuntimeExternalHttpEndpoint),
}

#[derive(Clone)]
pub struct RuntimeMcpBearer {
    secret_namespace: String,
    secret_name: String,
    authorization: HeaderValue,
}

impl PartialEq for RuntimeMcpBearer {
    fn eq(&self, other: &Self) -> bool {
        self.secret_namespace == other.secret_namespace
            && self.secret_name == other.secret_name
            && self.authorization == other.authorization
    }
}

impl Eq for RuntimeMcpBearer {}

impl RuntimeMcpBearer {
    /// Sensitive, ready-to-inject Authorization value for the future WP10 owner.
    pub fn authorization_header_value(&self) -> HeaderValue {
        self.authorization.clone()
    }
}

impl fmt::Debug for RuntimeMcpBearer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeMcpBearer")
            .field("authorization", &"[redacted]")
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeMcpBackend {
    pub uid: String,
    pub transport: RuntimeMcpTransport,
    pub target: RuntimeMcpTarget,
    pub bearer: Option<RuntimeMcpBearer>,
}

/// Canonical model selected by either its canonical name or an alias.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedModel {
    pub canonical_name: String,
    pub pricing: Option<ParsedAiModelPricing>,
    pub public: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeWildcardModel {
    pub pattern: String,
    pub prefix: String,
    pub model: ResolvedModel,
}

/// One usable credential in the configured provider credential pool.
#[derive(PartialEq, Eq)]
pub struct RuntimeAiCredential {
    pub name: String,
    pub weight: u32,
    pub credential: String,
    pub limits: Option<AiCredentialLimits>,
}

impl fmt::Debug for RuntimeAiCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeAiCredential")
            .field("name", &self.name)
            .field("weight", &self.weight)
            .field("limits", &self.limits)
            .finish()
    }
}

/// Cold-compiled credential-pool policy and its partial-good credentials.
pub struct RuntimeAiCredentialPool {
    pub redis_ref: Option<String>,
    pub on_redis_failure: OnRedisFailure,
    pub credentials: Vec<RuntimeAiCredential>,
    pub(crate) limiter_core: Arc<crate::plugins::common::redis_rate_limiter::RedisRateLimiter>,
}

impl fmt::Debug for RuntimeAiCredentialPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeAiCredentialPool")
            .field("redis_ref", &self.redis_ref)
            .field("on_redis_failure", &self.on_redis_failure)
            .field("credentials", &self.credentials)
            .finish()
    }
}

impl PartialEq for RuntimeAiCredentialPool {
    fn eq(&self, other: &Self) -> bool {
        self.redis_ref == other.redis_ref
            && self.on_redis_failure == other.on_redis_failure
            && self.credentials == other.credentials
    }
}

impl Eq for RuntimeAiCredentialPool {}

/// Request-ready AI backend configuration.
///
/// The original resource is intentionally not retained: ClientCache owns the
/// transport object, while this value contains only cold-compiled request data.
#[derive(PartialEq, Eq)]
pub struct RuntimeAiBackend {
    /// Stable identity for credential Redis keys and runtime state. Kubernetes
    /// resources use metadata.uid; UID-less config centers use the typed key.
    pub uid: String,
    pub provider: AiProvider,
    pub endpoint: RuntimeAiEndpoint,
    pub streaming: bool,
    pub max_output_tokens: Option<u32>,
    pub credential_pool: RuntimeAiCredentialPool,
    pub models: HashMap<String, ResolvedModel>,
    pub wildcard_models: Vec<RuntimeWildcardModel>,
    pub canonical_models: HashSet<String>,
    pub public_model_ids: BTreeSet<String>,
}

impl RuntimeAiBackend {
    pub fn resolve_model(&self, requested: &str) -> Option<&ResolvedModel> {
        self.models.get(requested).or_else(|| {
            self.wildcard_models
                .iter()
                .find(|alias| requested.starts_with(&alias.prefix))
                .map(|alias| &alias.model)
        })
    }

    pub fn has_canonical_model(&self, model: &str) -> bool {
        self.canonical_models.contains(model)
    }
}

impl fmt::Debug for RuntimeAiBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeAiBackend")
            .field("uid", &self.uid)
            .field("provider", &self.provider)
            .field("endpoint", &self.endpoint)
            .field("streaming", &self.streaming)
            .field("max_output_tokens", &self.max_output_tokens)
            .field("credential_pool", &self.credential_pool)
            .field("models", &self.models)
            .finish()
    }
}

/// One runtime generation entry. Keeping both variants in one enum prevents a
/// route from observing torn AI/A2A publications during hot reload.
#[derive(Debug, PartialEq, Eq)]
pub enum RuntimeEdgionBackend {
    Ai(Arc<RuntimeAiBackend>),
    A2a(Arc<RuntimeA2aBackend>),
    Mcp(Arc<RuntimeMcpBackend>),
}

struct Snapshot {
    generation: u64,
    backends: RuntimeMap,
    credential_outliers: Arc<super::AiCredentialOutlierStore>,
}

/// Request-stable AI-only view of the unified EdgionBackend snapshot.
///
/// A route with multiple AI backends captures this once, then resolves every
/// candidate from the same generation without repeated ArcSwap loads.
#[derive(Clone)]
pub struct AiBackendSnapshot {
    inner: Arc<Snapshot>,
}

impl AiBackendSnapshot {
    pub fn get_by_key(&self, key: &str) -> Option<Arc<RuntimeAiBackend>> {
        match self.inner.backends.get(key)?.as_ref() {
            RuntimeEdgionBackend::Ai(backend) => Some(Arc::clone(backend)),
            RuntimeEdgionBackend::A2a(_) | RuntimeEdgionBackend::Mcp(_) => None,
        }
    }

    /// Resolve several candidate keys from this one captured generation.
    /// Input order and missing slots are preserved for route-weight alignment.
    pub fn get_many_by_key<'a>(&self, keys: impl IntoIterator<Item = &'a str>) -> Vec<Option<Arc<RuntimeAiBackend>>> {
        keys.into_iter().map(|key| self.get_by_key(key)).collect()
    }

    pub fn credential_outlier_handle(
        &self,
        target: &PolicyTargetKey,
    ) -> Option<Arc<super::AiCredentialOutlierBackendHandle>> {
        self.inner.credential_outliers.current_handle(target)
    }
}

/// Request-stable typed view of every EdgionBackend variant.
#[derive(Clone)]
pub struct EdgionBackendSnapshot {
    inner: Arc<Snapshot>,
}

impl EdgionBackendSnapshot {
    pub fn generation(&self) -> u64 {
        self.inner.generation
    }

    pub fn get_by_key(&self, key: &str) -> Option<Arc<RuntimeEdgionBackend>> {
        self.inner.backends.get(key).cloned()
    }

    pub fn get_a2a_by_key(&self, key: &str) -> Option<Arc<RuntimeA2aBackend>> {
        match self.inner.backends.get(key)?.as_ref() {
            RuntimeEdgionBackend::A2a(backend) => Some(Arc::clone(backend)),
            RuntimeEdgionBackend::Ai(_) | RuntimeEdgionBackend::Mcp(_) => None,
        }
    }

    pub fn get_mcp_by_key(&self, key: &str) -> Option<Arc<RuntimeMcpBackend>> {
        match self.inner.backends.get(key)?.as_ref() {
            RuntimeEdgionBackend::Mcp(backend) => Some(Arc::clone(backend)),
            RuntimeEdgionBackend::Ai(_) | RuntimeEdgionBackend::A2a(_) => None,
        }
    }
}

/// Atomic store of all activatable EdgionBackend variants.
pub struct EdgionBackendStore {
    /// Production writer invariant: this handler is owned by exactly one
    /// ConfigClient cache, whose list/watch callbacks are serialized under the
    /// cache-data write lock. Therefore partial updates use load-clone-store
    /// without losing writes. A second production writer would require `rcu`
    /// or external serialization.
    snapshot: ArcSwap<Snapshot>,
    credential_outliers: Arc<super::AiCredentialOutlierStore>,
}

/// Compatibility name for existing AI consumers. It is the same unified store,
/// not a second store or publication path.
pub type AiBackendStore = EdgionBackendStore;

impl Default for EdgionBackendStore {
    fn default() -> Self {
        Self::new()
    }
}

impl EdgionBackendStore {
    pub fn new() -> Self {
        Self::with_credential_outlier_store(Arc::new(super::AiCredentialOutlierStore::new()))
    }

    fn with_credential_outlier_store(credential_outliers: Arc<super::AiCredentialOutlierStore>) -> Self {
        Self {
            snapshot: ArcSwap::from_pointee(Snapshot {
                generation: 0,
                backends: HashMap::new(),
                credential_outliers: Arc::clone(&credential_outliers),
            }),
            credential_outliers,
        }
    }

    fn target_key(key: &str) -> Option<PolicyTargetKey> {
        let (namespace, name) = key.split_once('/')?;
        if namespace.is_empty() || name.is_empty() {
            return None;
        }
        Some(PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionBackend,
            namespace: namespace.to_string(),
            name: name.to_string(),
        })
    }

    /// Mirror the backend's credential pool into the outlier store.
    ///
    /// `None` means the backend is no longer activatable — removed, or an
    /// update that failed to compile. The entry is dropped rather than emptied:
    /// it is only reachable through a live backend, so an emptied one is
    /// unreachable state that accumulates for the process lifetime. Requests
    /// already holding a captured generation keep it alive through their own
    /// `Arc`, which is what preserves in-flight ejection state.
    fn publish_credentials(&self, key: &str, backend: Option<&RuntimeEdgionBackend>) {
        let Some(target) = Self::target_key(key) else {
            return;
        };
        let Some(RuntimeEdgionBackend::Ai(backend)) = backend else {
            self.credential_outliers.remove_backend(&target);
            return;
        };
        let aliases = backend
            .credential_pool
            .credentials
            .iter()
            .map(|credential| credential.name.clone());
        self.credential_outliers.set_credentials(target, aliases);
    }

    /// One lock-free snapshot load plus one hash-table lookup.
    pub fn get_by_key(&self, key: &str) -> Option<Arc<RuntimeAiBackend>> {
        match self.snapshot.load().backends.get(key)?.as_ref() {
            RuntimeEdgionBackend::Ai(backend) => Some(Arc::clone(backend)),
            RuntimeEdgionBackend::A2a(_) | RuntimeEdgionBackend::Mcp(_) => None,
        }
    }

    /// Capture the variant-neutral generation for route selection and transport.
    pub fn edgion_backend_snapshot(&self) -> EdgionBackendSnapshot {
        EdgionBackendSnapshot {
            inner: self.snapshot.load_full(),
        }
    }

    pub fn get_a2a_by_key(&self, key: &str) -> Option<Arc<RuntimeA2aBackend>> {
        self.edgion_backend_snapshot().get_a2a_by_key(key)
    }

    /// Capture one immutable generation for a complete request/attempt plan.
    pub fn snapshot(&self) -> AiBackendSnapshot {
        AiBackendSnapshot {
            inner: self.snapshot.load_full(),
        }
    }

    /// Number of activatable entries. Invalid or unresolved resources are absent.
    pub fn count(&self) -> usize {
        self.snapshot
            .load()
            .backends
            .values()
            .filter(|backend| matches!(backend.as_ref(), RuntimeEdgionBackend::Ai(_)))
            .count()
    }

    pub fn keys(&self) -> Vec<String> {
        self.snapshot
            .load()
            .backends
            .iter()
            .filter_map(|(key, backend)| matches!(backend.as_ref(), RuntimeEdgionBackend::Ai(_)).then(|| key.clone()))
            .collect()
    }

    pub fn count_all(&self) -> usize {
        self.snapshot.load().backends.len()
    }

    pub fn keys_all(&self) -> Vec<String> {
        self.snapshot.load().backends.keys().cloned().collect()
    }

    fn replace_all(&self, resources: &HashMap<String, Arc<EdgionBackend>>) {
        let current = self.snapshot.load();
        let backends: RuntimeMap = resources
            .iter()
            .filter_map(|(key, resource)| {
                compile_backend_entry(key, resource, current.backends.get(key)).map(|backend| (key.clone(), backend))
            })
            .collect();
        if runtime_maps_share_entries(&current.backends, &backends) {
            return;
        }
        let adaptive_targets = changed_ai_backend_targets(&current.backends, &backends);
        for key in current.backends.keys() {
            if !backends.contains_key(key) {
                self.publish_credentials(key, None);
            }
        }
        for (key, backend) in &backends {
            self.publish_credentials(key, Some(backend));
        }
        self.snapshot.store(Arc::new(Snapshot {
            generation: current
                .generation
                .checked_add(1)
                .expect("EdgionBackend snapshot generation overflow"),
            backends,
            credential_outliers: Arc::clone(&self.credential_outliers),
        }));
        if std::ptr::eq(self, GLOBAL_EDGION_BACKEND_STORE.as_ref()) {
            let adaptive_store = super::get_adaptive_telemetry_store();
            for target in &adaptive_targets {
                adaptive_store.remove_target(target);
            }
            super::reconcile_adaptive_targets(&adaptive_targets);
        }
    }

    fn update(&self, add_or_update: HashMap<String, Arc<EdgionBackend>>, remove: &HashSet<String>) {
        let current = self.snapshot.load();
        let mut backends = current.backends.clone();
        for key in remove {
            backends.remove(key);
        }
        for (key, resource) in add_or_update {
            match compile_backend_entry(&key, &resource, backends.get(&key)) {
                Some(backend) => {
                    backends.insert(key, backend);
                }
                None => {
                    // An invalid update must deactivate the previous generation.
                    backends.remove(&key);
                }
            }
        }
        if runtime_maps_share_entries(&current.backends, &backends) {
            return;
        }
        let adaptive_targets = changed_ai_backend_targets(&current.backends, &backends);
        for key in current.backends.keys() {
            if !backends.contains_key(key) {
                self.publish_credentials(key, None);
            }
        }
        for (key, backend) in &backends {
            self.publish_credentials(key, Some(backend));
        }
        self.snapshot.store(Arc::new(Snapshot {
            generation: current
                .generation
                .checked_add(1)
                .expect("EdgionBackend snapshot generation overflow"),
            backends,
            credential_outliers: Arc::clone(&self.credential_outliers),
        }));
        if std::ptr::eq(self, GLOBAL_EDGION_BACKEND_STORE.as_ref()) {
            let adaptive_store = super::get_adaptive_telemetry_store();
            for target in &adaptive_targets {
                adaptive_store.remove_target(target);
            }
            super::reconcile_adaptive_targets(&adaptive_targets);
        }
    }
}

fn runtime_maps_share_entries(current: &RuntimeMap, next: &RuntimeMap) -> bool {
    current.len() == next.len()
        && next
            .iter()
            .all(|(key, backend)| current.get(key).is_some_and(|existing| Arc::ptr_eq(existing, backend)))
}

fn changed_ai_backend_targets(current: &RuntimeMap, next: &RuntimeMap) -> Vec<PolicyTargetKey> {
    current
        .keys()
        .chain(next.keys())
        .collect::<HashSet<_>>()
        .into_iter()
        .filter(|key| match (current.get(*key), next.get(*key)) {
            (Some(previous), Some(updated)) => match (previous.as_ref(), updated.as_ref()) {
                (RuntimeEdgionBackend::Ai(previous), RuntimeEdgionBackend::Ai(updated)) => {
                    !Arc::ptr_eq(previous, updated)
                }
                (RuntimeEdgionBackend::Ai(_), _) | (_, RuntimeEdgionBackend::Ai(_)) => true,
                _ => false,
            },
            (Some(previous), None) => matches!(previous.as_ref(), RuntimeEdgionBackend::Ai(_)),
            (None, Some(updated)) => matches!(updated.as_ref(), RuntimeEdgionBackend::Ai(_)),
            (None, None) => false,
        })
        .filter_map(|key| EdgionBackendStore::target_key(key))
        .collect()
}

pub fn get_global_ai_backend_store() -> Arc<AiBackendStore> {
    GLOBAL_EDGION_BACKEND_STORE.clone()
}

pub fn get_global_edgion_backend_store() -> Arc<EdgionBackendStore> {
    GLOBAL_EDGION_BACKEND_STORE.clone()
}

/// Borrow the process-global store without incrementing the outer Arc.
///
/// Multi-candidate request-path consumers should use this accessor, capture one
/// [`AiBackendSnapshot`], then resolve every candidate from that view.
#[inline]
pub fn global_ai_backend_store() -> &'static AiBackendStore {
    GLOBAL_EDGION_BACKEND_STORE.as_ref()
}

#[inline]
pub fn global_edgion_backend_store() -> &'static EdgionBackendStore {
    GLOBAL_EDGION_BACKEND_STORE.as_ref()
}

pub fn create_edgion_backend_handler() -> Box<dyn ConfHandler<EdgionBackend> + Send + Sync> {
    Box::new(get_global_edgion_backend_store())
}

pub fn count_ai_backends() -> usize {
    GLOBAL_EDGION_BACKEND_STORE.count()
}

#[cfg(test)]
static GLOBAL_AI_BACKEND_TEST_LOCK: LazyLock<parking_lot::Mutex<()>> = LazyLock::new(|| parking_lot::Mutex::new(()));

/// Exclusive guard for tests that mutate the process-global AI store.
///
/// Drop always clears the store, including during unwinding after a failed
/// assertion, so one test cannot leak resources into another.
#[cfg(test)]
pub struct GlobalAiBackendTestGuard {
    _lock: parking_lot::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for GlobalAiBackendTestGuard {
    fn drop(&mut self) {
        GLOBAL_EDGION_BACKEND_STORE.replace_all(&HashMap::new());
    }
}

#[cfg(test)]
pub fn lock_global_ai_backend_store_for_test() -> GlobalAiBackendTestGuard {
    let lock = GLOBAL_AI_BACKEND_TEST_LOCK.lock();
    GLOBAL_EDGION_BACKEND_STORE.replace_all(&HashMap::new());
    GlobalAiBackendTestGuard { _lock: lock }
}

impl ConfHandler<EdgionBackend> for EdgionBackendStore {
    fn full_set(&self, data: &HashMap<String, Arc<EdgionBackend>>) {
        tracing::info!(component = "edgion_backend_store", count = data.len(), "full set");
        self.replace_all(data);
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<EdgionBackend>>,
        update: HashMap<String, Arc<EdgionBackend>>,
        remove: HashSet<String>,
    ) {
        tracing::info!(
            component = "edgion_backend_store",
            add = add.len(),
            update = update.len(),
            remove = remove.len(),
            "partial update"
        );
        let mut add_or_update = add;
        add_or_update.extend(update);
        self.update(add_or_update, &remove);
    }
}

fn compile_backend(key: &str, resource: &EdgionBackend) -> Option<RuntimeEdgionBackend> {
    match try_compile_edgion_backend(key, resource) {
        Ok(backend) => Some(backend),
        Err(reason) => {
            tracing::warn!(
                component = "edgion_backend_store",
                key = key,
                reason = reason,
                "EdgionBackend omitted from activatable runtime index"
            );
            None
        }
    }
}

/// Compile a generation, retaining the prior Arc when normalized runtime data
/// is equivalent. This makes no-op resync behavior explicit and avoids future
/// per-backend state churn while captured request snapshots remain immutable.
fn compile_backend_entry(
    key: &str,
    resource: &EdgionBackend,
    previous: Option<&Arc<RuntimeEdgionBackend>>,
) -> Option<Arc<RuntimeEdgionBackend>> {
    let mut compiled = compile_backend(key, resource)?;
    if let Some(previous) = previous.filter(|previous| previous.as_ref() == &compiled) {
        return Some(previous.clone());
    }
    if let (RuntimeEdgionBackend::Ai(compiled_ai), Some(RuntimeEdgionBackend::Ai(previous_ai))) =
        (&mut compiled, previous.map(|backend| backend.as_ref()))
    {
        if previous_ai.credential_pool == compiled_ai.credential_pool {
            // Preserve request-shared limiter state for an otherwise changed AI
            // entry without giving A2A any AI-only side effects.
            Arc::get_mut(compiled_ai)
                .expect("newly compiled AI backend must be unique")
                .credential_pool
                .limiter_core = Arc::clone(&previous_ai.credential_pool.limiter_core);
        }
    }
    Some(Arc::new(compiled))
}

fn try_compile_edgion_backend(key: &str, resource: &EdgionBackend) -> Result<RuntimeEdgionBackend, &'static str> {
    resource.spec.validate().map_err(|_| "invalid configuration")?;
    match (&resource.spec.ai, &resource.spec.a2a, &resource.spec.mcp) {
        (Some(_), None, None) => try_compile_backend(key, resource)
            .map(Arc::new)
            .map(RuntimeEdgionBackend::Ai),
        (None, Some(_), None) => try_compile_a2a_backend(key, resource)
            .map(Arc::new)
            .map(RuntimeEdgionBackend::A2a),
        (None, None, Some(_)) => try_compile_mcp_backend(key, resource)
            .map(Arc::new)
            .map(RuntimeEdgionBackend::Mcp),
        _ => Err("invalid backend variant"),
    }
}

fn runtime_uid(key: &str, resource: &EdgionBackend) -> Result<String, &'static str> {
    if let Some(uid) = resource.metadata.uid.as_deref().filter(|uid| !uid.is_empty()) {
        return Ok(uid.to_string());
    }
    let (namespace, name) = key.split_once('/').ok_or("invalid resource key")?;
    if namespace.is_empty() || name.is_empty() {
        return Err("invalid resource key");
    }
    Ok(PolicyTargetKey {
        target_kind: PolicyTargetKind::EdgionBackend,
        namespace: namespace.to_string(),
        name: name.to_string(),
    }
    .to_string())
}

fn try_compile_a2a_backend(key: &str, resource: &EdgionBackend) -> Result<RuntimeA2aBackend, &'static str> {
    let a2a = resource.spec.a2a.as_ref().ok_or("missing A2A backend capability")?;
    let resolved = resource
        .spec
        .resolved_a2a
        .as_ref()
        .ok_or("missing resolved A2A target")?;
    let key_namespace = key
        .split_once('/')
        .map(|(namespace, _)| namespace)
        .ok_or("invalid resource key")?;
    let target = match (&a2a.target.service_ref, &resolved.target) {
        (Some(configured), ResolvedA2aTarget::Service { namespace, name, port })
            if configured.name == *name
                && configured.namespace.as_deref().unwrap_or(key_namespace) == namespace
                && configured.port == *port =>
        {
            RuntimeA2aTarget::Service {
                namespace: namespace.clone(),
                name: name.clone(),
                port: *port,
            }
        }
        (None, ResolvedA2aTarget::External { normalized_endpoint })
            if a2a.normalized_endpoint().as_deref() == Ok(normalized_endpoint.as_str()) =>
        {
            let endpoint = Url::parse(normalized_endpoint).map_err(|_| "invalid resolved A2A endpoint")?;
            RuntimeA2aTarget::External(
                compile_external_http_endpoint(&endpoint).ok_or("invalid resolved A2A endpoint")?,
            )
        }
        _ => return Err("resolved A2A target does not match the configured target"),
    };
    Ok(RuntimeA2aBackend {
        uid: runtime_uid(key, resource)?,
        target,
    })
}

fn try_compile_mcp_backend(key: &str, resource: &EdgionBackend) -> Result<RuntimeMcpBackend, &'static str> {
    let mcp = resource.spec.mcp.as_ref().ok_or("missing MCP backend capability")?;
    if mcp.transport.r#type != McpTransportType::StreamableHTTP {
        return Err("unsupported MCP transport");
    }
    let resolved = resource
        .spec
        .resolved_mcp
        .as_ref()
        .ok_or("missing resolved MCP target")?;
    let key_namespace = key
        .split_once('/')
        .map(|(namespace, _)| namespace)
        .ok_or("invalid resource key")?;
    let target = match (&mcp.target.service_ref, &resolved.target) {
        (
            Some(configured),
            ResolvedMcpTarget::Service {
                namespace,
                name,
                port,
                path,
            },
        ) if configured.name == *name
            && configured.namespace.as_deref().unwrap_or(key_namespace) == namespace
            && configured.port == *port
            && mcp.normalized_service_path().as_deref() == Ok(path.as_str()) =>
        {
            RuntimeMcpTarget::Service {
                namespace: namespace.clone(),
                name: name.clone(),
                port: *port,
                path: path.clone(),
            }
        }
        (None, ResolvedMcpTarget::External { normalized_endpoint })
            if mcp.normalized_endpoint().as_deref() == Ok(normalized_endpoint.as_str()) =>
        {
            let endpoint = Url::parse(normalized_endpoint).map_err(|_| "invalid resolved MCP endpoint")?;
            RuntimeMcpTarget::External(
                compile_external_http_endpoint(&endpoint).ok_or("invalid resolved MCP endpoint")?,
            )
        }
        _ => return Err("resolved MCP target does not match the configured target"),
    };

    let bearer = match mcp.authentication.as_ref() {
        None => None,
        Some(authentication) => {
            let secret_ref = &authentication.bearer_token.secret_ref;
            let secret = authentication
                .bearer_token
                .secret
                .as_ref()
                .ok_or("missing resolved MCP bearer Secret")?;
            let expected_namespace = secret_ref.namespace.as_deref().unwrap_or(key_namespace);
            if secret.metadata.name.as_deref() != Some(secret_ref.name.as_str())
                || secret.metadata.namespace.as_deref() != Some(expected_namespace)
            {
                return Err("resolved MCP bearer Secret does not match the configured reference");
            }
            let resolver = BearerResolver;
            let secret_type = secret.type_.as_deref().unwrap_or("");
            if !resolver.allowed_secret_types().contains(&secret_type) {
                return Err("invalid MCP bearer Secret type");
            }
            let material = resolver
                .parse(secret.data.as_ref().ok_or("missing MCP bearer Secret data")?)
                .map_err(|_| "invalid MCP bearer Secret data")?;
            let mut authorization = HeaderValue::from_str(&format!("Bearer {}", material.token))
                .map_err(|_| "invalid MCP bearer authorization value")?;
            authorization.set_sensitive(true);
            Some(RuntimeMcpBearer {
                secret_namespace: expected_namespace.to_string(),
                secret_name: secret_ref.name.clone(),
                authorization,
            })
        }
    };

    Ok(RuntimeMcpBackend {
        uid: runtime_uid(key, resource)?,
        transport: RuntimeMcpTransport::StreamableHttp20260728,
        target,
        bearer,
    })
}

fn try_compile_backend(key: &str, resource: &EdgionBackend) -> Result<RuntimeAiBackend, &'static str> {
    let ai = resource.spec.ai.as_ref().ok_or("missing AI backend capability")?;

    let uid = runtime_uid(key, resource)?;

    let endpoint = ai.effective_endpoint().map_err(|_| "invalid endpoint")?;
    let endpoint = compile_endpoint(&endpoint).ok_or("invalid endpoint")?;
    let resolver = AiApiKeyResolver;
    let credentials = ai
        .credential_pool
        .credentials
        .iter()
        .filter_map(|configured| {
            let secret = configured.credential.secret.as_ref()?;
            let secret_type = secret.type_.as_deref().unwrap_or("");
            if !resolver.allowed_secret_types().contains(&secret_type) {
                return None;
            }
            let data = secret.data.as_ref()?;
            let credential = resolver.parse(data).ok()?.api_key;
            Some(RuntimeAiCredential {
                name: configured.name.clone(),
                weight: configured.weight,
                credential,
                limits: configured.limits.clone(),
            })
        })
        .collect::<Vec<_>>();
    if credentials.is_empty() {
        return Err("no usable credentials");
    }
    let credential_pool = RuntimeAiCredentialPool {
        redis_ref: ai.credential_pool.redis_ref.clone(),
        on_redis_failure: ai.credential_pool.on_redis_failure.clone(),
        credentials,
        limiter_core: Arc::new(crate::plugins::common::redis_rate_limiter::RedisRateLimiter::new()),
    };

    let canonical_pricing: HashMap<&str, Option<ParsedAiModelPricing>> = ai
        .models
        .iter()
        .map(|model| {
            let pricing = model
                .pricing
                .as_ref()
                .map(|pricing| pricing.parsed())
                .transpose()
                .map_err(|_| "invalid model pricing")?;
            Ok((model.name.as_str(), pricing))
        })
        .collect::<Result<_, &'static str>>()?;
    let catalog = ai.model_catalog().map_err(|_| "invalid model catalog")?;
    let models = catalog
        .exact
        .into_iter()
        .map(|(name, entry)| {
            let pricing = canonical_pricing.get(entry.canonical_name.as_str()).copied().flatten();
            (
                name,
                ResolvedModel {
                    canonical_name: entry.canonical_name,
                    pricing,
                    public: entry.public,
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let wildcard_models = catalog
        .wildcards
        .into_iter()
        .map(|alias| RuntimeWildcardModel {
            pattern: alias.pattern,
            prefix: alias.prefix,
            model: ResolvedModel {
                pricing: canonical_pricing
                    .get(alias.entry.canonical_name.as_str())
                    .copied()
                    .flatten(),
                canonical_name: alias.entry.canonical_name,
                public: alias.entry.public,
            },
        })
        .collect::<Vec<_>>();
    let canonical_models = ai.models.iter().map(|model| model.name.clone()).collect();
    let public_model_ids = ai
        .models
        .iter()
        .filter(|model| model.public)
        .flat_map(|model| std::iter::once(model.name.clone()).chain(model.aliases.iter().cloned()))
        .collect();

    Ok(RuntimeAiBackend {
        uid,
        provider: ai.provider,
        endpoint,
        streaming: ai.defaults.streaming,
        max_output_tokens: ai.defaults.max_output_tokens,
        credential_pool,
        models,
        wildcard_models,
        canonical_models,
        public_model_ids,
    })
}

fn compile_endpoint(endpoint: &Url) -> Option<RuntimeAiEndpoint> {
    let host = endpoint.host()?;
    let (domain, serialized_host) = match host {
        Host::Domain(domain) => (domain.to_string(), domain.to_string()),
        Host::Ipv4(ip) => (ip.to_string(), ip.to_string()),
        Host::Ipv6(ip) => (ip.to_string(), format!("[{ip}]")),
    };
    let port = endpoint.port_or_known_default()?;
    let use_tls = endpoint.scheme() == "https";
    let authority = match endpoint.port() {
        Some(explicit_port) => format!("{serialized_host}:{explicit_port}"),
        None => serialized_host,
    };
    let path_prefix = match endpoint.path().trim_end_matches('/') {
        "" => String::new(),
        path => path.to_string(),
    };

    Some(RuntimeAiEndpoint {
        sni: domain.clone(),
        domain,
        port,
        use_tls,
        host: authority,
        path_prefix,
    })
}

fn compile_external_http_endpoint(endpoint: &Url) -> Option<RuntimeExternalHttpEndpoint> {
    let endpoint = compile_endpoint(endpoint)?;
    Some(RuntimeExternalHttpEndpoint {
        domain: endpoint.domain,
        port: endpoint.port,
        use_tls: endpoint.use_tls,
        sni: endpoint.sni,
        authority: endpoint.host,
        path_prefix: endpoint.path_prefix,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
    use edgion_resources::resources::edgion_backend::{
        A2aBackendSpec, A2aBackendTarget, AiBackendDefaults, AiBackendSpec, AiCredential, AiCredentialPool, AiModel,
        AiModelPricing, BackendServiceReference, EdgionBackendSpec, McpAuthentication, McpBackendSpec,
        McpBackendTarget, McpTransport, ResolvedA2aBackend, ResolvedMcpBackend,
    };
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    fn resource(name: &str, endpoint: &str, credential: Option<&[u8]>) -> Arc<EdgionBackend> {
        let secret = credential.map(|value| Secret {
            data: Some(BTreeMap::from([("api-key".to_string(), ByteString(value.to_vec()))])),
            ..Default::default()
        });
        let spec = EdgionBackendSpec {
            ai: Some(AiBackendSpec {
                provider: AiProvider::OpenAICompatible,
                endpoint: Some(endpoint.to_string()),
                allow_insecure_http: false,
                credential_pool: AiCredentialPool {
                    redis_ref: None,
                    on_redis_failure: OnRedisFailure::Deny,
                    credentials: vec![AiCredential {
                        name: "primary".to_string(),
                        weight: 1,
                        credential: SecretSlot {
                            secret_ref: SecretObjectReference {
                                group: None,
                                kind: None,
                                name: "provider-key".to_string(),
                                namespace: None,
                            },
                            secret,
                        },
                        limits: None,
                    }],
                },
                models: vec![AiModel {
                    name: "model-a".to_string(),
                    aliases: vec!["fast".to_string()],
                    public: true,
                    pricing: None,
                }],
                defaults: AiBackendDefaults {
                    streaming: true,
                    max_output_tokens: Some(2048),
                },
            }),
            a2a: None,
            mcp: None,
            resolved_a2a: None,
            resolved_mcp: None,
            reconcile_outcome: None,
        };
        let mut resource = EdgionBackend::new(name, spec);
        resource.metadata.namespace = Some("default".to_string());
        Arc::new(resource)
    }

    fn a2a_service_resource(name: &str, resolved: bool) -> Arc<EdgionBackend> {
        let spec = EdgionBackendSpec {
            ai: None,
            a2a: Some(A2aBackendSpec {
                target: A2aBackendTarget {
                    service_ref: Some(BackendServiceReference {
                        name: "agent-service".to_string(),
                        namespace: Some("agents".to_string()),
                        port: 8080,
                    }),
                    endpoint: None,
                },
                allow_insecure_http: false,
            }),
            mcp: None,
            resolved_a2a: resolved.then(|| ResolvedA2aBackend {
                target: ResolvedA2aTarget::Service {
                    namespace: "agents".to_string(),
                    name: "agent-service".to_string(),
                    port: 8080,
                },
            }),
            resolved_mcp: None,
            reconcile_outcome: None,
        };
        let mut resource = EdgionBackend::new(name, spec);
        resource.metadata.namespace = Some("default".to_string());
        Arc::new(resource)
    }

    fn a2a_external_resource(name: &str, endpoint: &str, resolved_endpoint: &str) -> Arc<EdgionBackend> {
        let spec = EdgionBackendSpec {
            ai: None,
            a2a: Some(A2aBackendSpec {
                target: A2aBackendTarget {
                    service_ref: None,
                    endpoint: Some(endpoint.to_string()),
                },
                allow_insecure_http: endpoint.starts_with("http://"),
            }),
            mcp: None,
            resolved_a2a: Some(ResolvedA2aBackend {
                target: ResolvedA2aTarget::External {
                    normalized_endpoint: resolved_endpoint.to_string(),
                },
            }),
            resolved_mcp: None,
            reconcile_outcome: None,
        };
        let mut resource = EdgionBackend::new(name, spec);
        resource.metadata.namespace = Some("default".to_string());
        Arc::new(resource)
    }

    fn mcp_external_resource(name: &str, endpoint: &str, token: Option<&[u8]>) -> Arc<EdgionBackend> {
        let authentication = token.map(|token| McpAuthentication {
            bearer_token: SecretSlot {
                secret_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "mcp-bearer".to_string(),
                    namespace: None,
                },
                secret: Some(Secret {
                    metadata: ObjectMeta {
                        name: Some("mcp-bearer".to_string()),
                        namespace: Some("default".to_string()),
                        ..Default::default()
                    },
                    data: Some(BTreeMap::from([(
                        "bearerToken".to_string(),
                        ByteString(token.to_vec()),
                    )])),
                    ..Default::default()
                }),
            },
        });
        let normalized = endpoint.trim_end_matches('/').to_string();
        let spec = EdgionBackendSpec {
            ai: None,
            a2a: None,
            mcp: Some(McpBackendSpec {
                target: McpBackendTarget {
                    service_ref: None,
                    endpoint: Some(endpoint.to_string()),
                    path: None,
                },
                allow_insecure_http: endpoint.starts_with("http://"),
                transport: McpTransport {
                    r#type: McpTransportType::StreamableHTTP,
                },
                authentication,
            }),
            resolved_a2a: None,
            resolved_mcp: Some(ResolvedMcpBackend {
                target: ResolvedMcpTarget::External {
                    normalized_endpoint: normalized,
                },
            }),
            reconcile_outcome: None,
        };
        let mut resource = EdgionBackend::new(name, spec);
        resource.metadata.namespace = Some("default".to_string());
        Arc::new(resource)
    }

    fn with_mcp_secret_identity(
        resource: Arc<EdgionBackend>,
        reference_namespace: Option<&str>,
        resolved_name: &str,
        resolved_namespace: &str,
    ) -> Arc<EdgionBackend> {
        let mut resource = (*resource).clone();
        let authentication = resource
            .spec
            .mcp
            .as_mut()
            .and_then(|mcp| mcp.authentication.as_mut())
            .expect("MCP bearer authentication");
        authentication.bearer_token.secret_ref.namespace = reference_namespace.map(str::to_string);
        let secret = authentication
            .bearer_token
            .secret
            .as_mut()
            .expect("resolved MCP bearer Secret");
        secret.metadata.name = Some(resolved_name.to_string());
        secret.metadata.namespace = Some(resolved_namespace.to_string());
        Arc::new(resource)
    }

    fn mcp_service_resource(name: &str, path: &str) -> Arc<EdgionBackend> {
        let spec = EdgionBackendSpec {
            ai: None,
            a2a: None,
            mcp: Some(McpBackendSpec {
                target: McpBackendTarget {
                    service_ref: Some(BackendServiceReference {
                        name: "mcp-service".to_string(),
                        namespace: Some("tools".to_string()),
                        port: 9090,
                    }),
                    endpoint: None,
                    path: Some(path.to_string()),
                },
                allow_insecure_http: false,
                transport: McpTransport {
                    r#type: McpTransportType::StreamableHTTP,
                },
                authentication: None,
            }),
            resolved_a2a: None,
            resolved_mcp: Some(ResolvedMcpBackend {
                target: ResolvedMcpTarget::Service {
                    namespace: "tools".to_string(),
                    name: "mcp-service".to_string(),
                    port: 9090,
                    path: path.to_string(),
                },
            }),
            reconcile_outcome: None,
        };
        let mut resource = EdgionBackend::new(name, spec);
        resource.metadata.namespace = Some("default".to_string());
        Arc::new(resource)
    }

    #[test]
    fn starts_empty() {
        let store = AiBackendStore::new();
        assert_eq!(store.count(), 0);
        assert!(store.keys().is_empty());
        assert!(store.get_by_key("default/missing").is_none());
    }

    #[test]
    fn mcp_external_compiles_and_secret_rotation_controls_generation() {
        let store = EdgionBackendStore::new();
        let key = "default/tools";
        let initial = mcp_external_resource("tools", "https://tools.example.test/mcp/", Some(b"token-a"));
        store.full_set(&HashMap::from([(key.to_string(), initial)]));
        let first_snapshot = store.edgion_backend_snapshot();
        let first = first_snapshot.get_mcp_by_key(key).unwrap();
        assert_eq!(first_snapshot.generation(), 1);
        let RuntimeMcpTarget::External(endpoint) = &first.target else {
            panic!("expected external MCP target");
        };
        assert_eq!(endpoint.authority, "tools.example.test");
        assert_eq!(endpoint.path_prefix, "/mcp");
        assert_eq!(
            first.bearer.as_ref().unwrap().authorization_header_value().as_bytes(),
            b"Bearer token-a"
        );
        assert!(!format!("{:?}", first.bearer.as_ref().unwrap()).contains("token-a"));

        store.full_set(&HashMap::from([(
            key.to_string(),
            mcp_external_resource("tools", "https://tools.example.test/mcp/", Some(b"token-a")),
        )]));
        let equivalent_snapshot = store.edgion_backend_snapshot();
        let equivalent = equivalent_snapshot.get_mcp_by_key(key).unwrap();
        assert_eq!(equivalent_snapshot.generation(), first_snapshot.generation());
        assert!(Arc::ptr_eq(&first, &equivalent));

        store.partial_update(
            HashMap::new(),
            HashMap::from([(
                key.to_string(),
                mcp_external_resource("tools", "https://tools.example.test/mcp/", Some(b"token-b")),
            )]),
            HashSet::new(),
        );
        let rotated_snapshot = store.edgion_backend_snapshot();
        let rotated = rotated_snapshot.get_mcp_by_key(key).unwrap();
        assert_eq!(rotated_snapshot.generation(), first_snapshot.generation() + 1);
        assert!(!Arc::ptr_eq(&first, &rotated));
        assert_eq!(
            rotated.bearer.as_ref().unwrap().authorization_header_value().as_bytes(),
            b"Bearer token-b"
        );
    }

    #[test]
    fn mcp_service_compiles_from_exact_controller_proof() {
        let store = EdgionBackendStore::new();
        let key = "default/tools";
        store.full_set(&HashMap::from([(
            key.to_string(),
            mcp_service_resource("tools", "/mcp"),
        )]));

        let runtime = store.edgion_backend_snapshot().get_mcp_by_key(key).unwrap();
        assert_eq!(
            runtime.target,
            RuntimeMcpTarget::Service {
                namespace: "tools".to_string(),
                name: "mcp-service".to_string(),
                port: 9090,
                path: "/mcp".to_string(),
            }
        );
        assert_eq!(runtime.transport, RuntimeMcpTransport::StreamableHttp20260728);
        assert!(runtime.bearer.is_none());

        let mut forged = (*mcp_service_resource("tools", "/mcp")).clone();
        forged.spec.resolved_mcp = Some(ResolvedMcpBackend {
            target: ResolvedMcpTarget::Service {
                namespace: "tools".to_string(),
                name: "mcp-service".to_string(),
                port: 9090,
                path: "/other".to_string(),
            },
        });
        store.partial_update(
            HashMap::new(),
            HashMap::from([(key.to_string(), Arc::new(forged))]),
            HashSet::new(),
        );
        assert!(store.edgion_backend_snapshot().get_mcp_by_key(key).is_none());
    }

    #[test]
    fn empty_and_invalid_noop_updates_preserve_snapshot_identity_and_generation() {
        let store = EdgionBackendStore::new();
        let initial = store.edgion_backend_snapshot();

        store.partial_update(HashMap::new(), HashMap::new(), HashSet::new());
        let empty_noop = store.edgion_backend_snapshot();
        assert_eq!(empty_noop.generation(), 0);
        assert!(Arc::ptr_eq(&initial.inner, &empty_noop.inner));

        store.partial_update(
            HashMap::new(),
            HashMap::from([(
                "default/invalid".to_string(),
                mcp_external_resource("invalid", "https://tools.example.test/mcp", Some(b"bad token")),
            )]),
            HashSet::new(),
        );
        let invalid_noop = store.edgion_backend_snapshot();
        assert_eq!(invalid_noop.generation(), 0);
        assert!(Arc::ptr_eq(&initial.inner, &invalid_noop.inner));
    }

    #[test]
    fn invalid_mcp_credential_withdraws_previous_runtime() {
        let store = EdgionBackendStore::new();
        let key = "default/tools";
        store.full_set(&HashMap::from([(
            key.to_string(),
            mcp_external_resource("tools", "https://tools.example.test/mcp", Some(b"valid-token")),
        )]));
        assert!(store.edgion_backend_snapshot().get_mcp_by_key(key).is_some());

        store.partial_update(
            HashMap::new(),
            HashMap::from([(
                key.to_string(),
                mcp_external_resource("tools", "https://tools.example.test/mcp", Some(b"bad token")),
            )]),
            HashSet::new(),
        );
        assert!(store.edgion_backend_snapshot().get_mcp_by_key(key).is_none());
    }

    #[test]
    fn mcp_bearer_secret_identity_is_exact_and_mismatch_withdraws_runtime() {
        let store = EdgionBackendStore::new();
        let key = "default/tools";
        let same_namespace = mcp_external_resource("tools", "https://tools.example.test/mcp", Some(b"valid-token"));
        store.full_set(&HashMap::from([(key.to_string(), same_namespace)]));
        let initial = store.edgion_backend_snapshot();
        let initial_runtime = initial.get_mcp_by_key(key).unwrap();

        store.partial_update(
            HashMap::new(),
            HashMap::from([(
                key.to_string(),
                mcp_external_resource("tools", "https://tools.example.test/mcp", Some(b"valid-token")),
            )]),
            HashSet::new(),
        );
        let equivalent = store.edgion_backend_snapshot();
        assert_eq!(equivalent.generation(), initial.generation());
        assert!(Arc::ptr_eq(&initial.inner, &equivalent.inner));
        assert!(Arc::ptr_eq(&initial_runtime, &equivalent.get_mcp_by_key(key).unwrap()));

        let cross_namespace = with_mcp_secret_identity(
            mcp_external_resource("tools", "https://tools.example.test/mcp", Some(b"valid-token")),
            Some("credentials"),
            "mcp-bearer",
            "credentials",
        );
        store.partial_update(
            HashMap::new(),
            HashMap::from([(key.to_string(), cross_namespace)]),
            HashSet::new(),
        );
        let changed_identity = store.edgion_backend_snapshot();
        assert_eq!(changed_identity.generation(), equivalent.generation() + 1);
        assert!(!Arc::ptr_eq(&equivalent.inner, &changed_identity.inner));
        assert!(!Arc::ptr_eq(
            &initial_runtime,
            &changed_identity.get_mcp_by_key(key).unwrap()
        ));

        let wrong_name = with_mcp_secret_identity(
            mcp_external_resource("tools", "https://tools.example.test/mcp", Some(b"valid-token")),
            None,
            "other-bearer",
            "default",
        );
        store.partial_update(
            HashMap::new(),
            HashMap::from([(key.to_string(), wrong_name)]),
            HashSet::new(),
        );
        assert!(store.edgion_backend_snapshot().get_mcp_by_key(key).is_none());

        store.partial_update(
            HashMap::new(),
            HashMap::from([(
                key.to_string(),
                mcp_external_resource("tools", "https://tools.example.test/mcp", Some(b"valid-token")),
            )]),
            HashSet::new(),
        );
        assert!(store.edgion_backend_snapshot().get_mcp_by_key(key).is_some());

        let wrong_namespace = with_mcp_secret_identity(
            mcp_external_resource("tools", "https://tools.example.test/mcp", Some(b"valid-token")),
            Some("credentials"),
            "mcp-bearer",
            "default",
        );
        store.partial_update(
            HashMap::new(),
            HashMap::from([(key.to_string(), wrong_namespace)]),
            HashSet::new(),
        );
        assert!(store.edgion_backend_snapshot().get_mcp_by_key(key).is_none());
    }

    #[test]
    fn mcp_to_ai_and_a2a_variant_switches_are_atomic() {
        let store = EdgionBackendStore::new();
        let key = "default/tools";
        store.full_set(&HashMap::from([(
            key.to_string(),
            mcp_external_resource("tools", "https://tools.example.test/mcp", None),
        )]));
        assert!(store.edgion_backend_snapshot().get_mcp_by_key(key).is_some());

        store.partial_update(
            HashMap::new(),
            HashMap::from([(
                key.to_string(),
                resource("tools", "https://ai.example.test", Some(b"key")),
            )]),
            HashSet::new(),
        );
        let ai = store.edgion_backend_snapshot();
        assert!(ai.get_mcp_by_key(key).is_none());
        assert!(matches!(
            ai.get_by_key(key).as_deref(),
            Some(RuntimeEdgionBackend::Ai(_))
        ));

        store.partial_update(
            HashMap::new(),
            HashMap::from([(
                key.to_string(),
                mcp_external_resource("tools", "https://tools.example.test/mcp", None),
            )]),
            HashSet::new(),
        );
        store.partial_update(
            HashMap::new(),
            HashMap::from([(key.to_string(), a2a_service_resource("tools", true))]),
            HashSet::new(),
        );
        let a2a = store.edgion_backend_snapshot();
        assert!(a2a.get_mcp_by_key(key).is_none());
        assert!(a2a.get_a2a_by_key(key).is_some());
    }

    #[test]
    fn a2a_service_and_external_targets_compile_from_controller_proof() {
        let store = EdgionBackendStore::new();
        store.full_set(&HashMap::from([
            (
                "default/service-agent".to_string(),
                a2a_service_resource("service-agent", true),
            ),
            (
                "default/external-agent".to_string(),
                a2a_external_resource(
                    "external-agent",
                    "https://agent.example.com:8443/base/",
                    "https://agent.example.com:8443/base",
                ),
            ),
        ]));

        assert_eq!(store.count(), 0, "A2A entries must not enter the AI view");
        assert_eq!(store.count_all(), 2);
        let snapshot = store.edgion_backend_snapshot();
        let service = snapshot.get_a2a_by_key("default/service-agent").unwrap();
        assert_eq!(
            service.target,
            RuntimeA2aTarget::Service {
                namespace: "agents".to_string(),
                name: "agent-service".to_string(),
                port: 8080,
            }
        );
        let external = snapshot.get_a2a_by_key("default/external-agent").unwrap();
        let RuntimeA2aTarget::External(external) = &external.target else {
            panic!("expected external target");
        };
        assert_eq!(external.domain, "agent.example.com");
        assert_eq!(external.port, 8443);
        assert!(external.use_tls);
        assert_eq!(external.sni, "agent.example.com");
        assert_eq!(external.authority, "agent.example.com:8443");
        assert_eq!(external.path_prefix, "/base");
    }

    #[test]
    fn request_pin_keeps_variant_generation_stable_across_hot_reload() {
        let store = EdgionBackendStore::new();
        let key = "default/agent".to_string();

        store.full_set(&HashMap::from([(
            key.clone(),
            resource("agent", "https://ai.example", Some(b"test-key")),
        )]));
        let old_ai = store.edgion_backend_snapshot();
        store.full_set(&HashMap::from([(key.clone(), a2a_service_resource("agent", true))]));
        let new_a2a = store.edgion_backend_snapshot();
        let selected = crate::routes::http::proxy_http::backend_snapshot_for_request(Some(&old_ai), || new_a2a);
        assert!(matches!(
            selected.get_by_key(&key).as_deref(),
            Some(RuntimeEdgionBackend::Ai(_))
        ));
        let selected = crate::routes::http::proxy_http::backend_snapshot_for_request(Some(&old_ai), || {
            panic!("pinned requests must not load the current generation")
        });
        assert!(matches!(
            selected.get_by_key(&key).as_deref(),
            Some(RuntimeEdgionBackend::Ai(_))
        ));

        let old_a2a = store.edgion_backend_snapshot();
        store.full_set(&HashMap::from([(
            key.clone(),
            resource("agent", "https://ai.example", Some(b"test-key")),
        )]));
        let new_ai = store.edgion_backend_snapshot();
        let selected = crate::routes::http::proxy_http::backend_snapshot_for_request(Some(&old_a2a), || new_ai);
        assert!(matches!(
            selected.get_by_key(&key).as_deref(),
            Some(RuntimeEdgionBackend::A2a(_))
        ));
    }

    #[test]
    fn unresolved_or_mismatched_a2a_proof_withdraws_previous_generation() {
        let store = EdgionBackendStore::new();
        let key = "default/agent";
        store.full_set(&HashMap::from([(key.to_string(), a2a_service_resource("agent", true))]));
        assert!(store.get_a2a_by_key(key).is_some());

        store.partial_update(
            HashMap::new(),
            HashMap::from([(key.to_string(), a2a_service_resource("agent", false))]),
            HashSet::new(),
        );
        assert!(store.get_a2a_by_key(key).is_none());

        store.full_set(&HashMap::from([(
            key.to_string(),
            a2a_external_resource(
                "agent",
                "https://agent.example.com/base",
                "https://other.example.com/base",
            ),
        )]));
        assert!(store.get_a2a_by_key(key).is_none());
    }

    #[test]
    fn ai_to_a2a_switch_is_atomic_and_removes_ai_only_side_effects() {
        let store = EdgionBackendStore::new();
        let key = "default/agent";
        store.full_set(&HashMap::from([(
            key.to_string(),
            resource("agent", "https://ai.example.com", Some(b"key")),
        )]));
        let old = store.edgion_backend_snapshot();
        assert!(old.get_by_key(key).is_some());
        assert_eq!(store.credential_outliers.count(), 1);

        store.partial_update(
            HashMap::new(),
            HashMap::from([(key.to_string(), a2a_service_resource("agent", true))]),
            HashSet::new(),
        );
        let current = store.edgion_backend_snapshot();
        assert!(matches!(
            old.get_by_key(key).unwrap().as_ref(),
            RuntimeEdgionBackend::Ai(_)
        ));
        assert!(matches!(
            current.get_by_key(key).unwrap().as_ref(),
            RuntimeEdgionBackend::A2a(_)
        ));
        assert!(store.get_by_key(key).is_none());
        assert_eq!(store.credential_outliers.count(), 0);
    }

    #[test]
    fn equivalent_a2a_resync_retains_runtime_arc() {
        let store = EdgionBackendStore::new();
        let key = "default/agent";
        store.full_set(&HashMap::from([(
            key.to_string(),
            a2a_external_resource(
                "agent",
                "https://agent.example.com/base/",
                "https://agent.example.com/base",
            ),
        )]));
        let before = store.edgion_backend_snapshot().get_by_key(key).unwrap();
        store.full_set(&HashMap::from([(
            key.to_string(),
            a2a_external_resource(
                "agent",
                "https://agent.example.com/base",
                "https://agent.example.com/base",
            ),
        )]));
        let after = store.edgion_backend_snapshot().get_by_key(key).unwrap();
        assert!(Arc::ptr_eq(&before, &after));
    }

    #[test]
    fn full_set_atomically_publishes_compiled_service() {
        let store = AiBackendStore::new();
        let resources = HashMap::from([(
            "default/provider".to_string(),
            resource(
                "provider",
                "https://ai.example.com:8443/openai/v1/",
                Some(b"secret-value"),
            ),
        )]);
        store.full_set(&resources);

        assert_eq!(store.count(), 1);
        assert_eq!(store.keys(), vec!["default/provider"]);
        let backend = store.get_by_key("default/provider").expect("compiled backend");
        assert_eq!(backend.uid, "edgion.io/EdgionBackend/default/provider");
        assert_eq!(backend.endpoint.domain, "ai.example.com");
        assert_eq!(backend.endpoint.port, 8443);
        assert!(backend.endpoint.use_tls);
        assert_eq!(backend.endpoint.sni, "ai.example.com");
        assert_eq!(backend.endpoint.host, "ai.example.com:8443");
        assert_eq!(backend.endpoint.path_prefix, "/openai/v1");
        assert!(backend.streaming);
        assert_eq!(backend.max_output_tokens, Some(2048));
        assert_eq!(backend.credential_pool.credentials.len(), 1);
        assert_eq!(backend.credential_pool.credentials[0].name, "primary");
        assert_eq!(backend.credential_pool.credentials[0].credential, "secret-value");
        assert_eq!(backend.models["model-a"].canonical_name, "model-a");
        assert_eq!(backend.models["fast"].canonical_name, "model-a");
    }

    #[test]
    fn runtime_catalog_resolves_longest_wildcard_and_visibility() {
        let mut configured = resource("provider", "https://ai.example.com", Some(b"secret-value"));
        let ai = Arc::get_mut(&mut configured).unwrap().spec.ai.as_mut().unwrap();
        ai.models = vec![
            AiModel {
                name: "broad".to_string(),
                aliases: vec!["gpt*".to_string()],
                public: false,
                pricing: None,
            },
            AiModel {
                name: "specific".to_string(),
                aliases: vec!["gpt-4*".to_string()],
                public: true,
                pricing: None,
            },
        ];
        let store = AiBackendStore::new();
        store.replace_all(&HashMap::from([("default/provider".to_string(), configured)]));
        let backend = store.get_by_key("default/provider").unwrap();
        assert_eq!(backend.resolve_model("gpt-4.1").unwrap().canonical_name, "specific");
        assert_eq!(backend.resolve_model("gpt-3").unwrap().canonical_name, "broad");
        assert!(backend.public_model_ids.contains("gpt-4*"));
        assert!(!backend.public_model_ids.contains("gpt*"));
    }

    #[test]
    fn model_pricing_is_compiled_once_and_shared_by_aliases() {
        let mut configured = resource("provider", "https://ai.example.com", Some(b"secret-value"));
        Arc::get_mut(&mut configured).unwrap().spec.ai.as_mut().unwrap().models[0].pricing = Some(AiModelPricing {
            input_per_million_usd: "15".to_string(),
            output_per_million_usd: "75".to_string(),
            cache_read_input_per_million_usd: Some("1.5".to_string()),
            cache_write_input_per_million_usd: None,
        });
        let backend = try_compile_backend("default/provider", &configured).unwrap();
        let canonical = backend.models["model-a"].pricing.unwrap();
        let alias = backend.models["fast"].pricing.unwrap();
        assert_eq!(canonical, alias);
        assert_eq!(canonical.input.get(), 15_000_000);
        assert_eq!(canonical.cache_read_input.get(), 1_500_000);
        assert_eq!(canonical.cache_write_input, canonical.input);
    }

    #[test]
    fn partial_update_adds_updates_removes_and_deactivates_invalid_generation() {
        let store = AiBackendStore::new();
        store.partial_update(
            HashMap::from([(
                "default/a".to_string(),
                resource("a", "https://a.example.com", Some(b"a-key")),
            )]),
            HashMap::new(),
            HashSet::new(),
        );
        assert_eq!(
            store.get_by_key("default/a").unwrap().credential_pool.credentials[0].credential,
            "a-key"
        );

        store.partial_update(
            HashMap::from([(
                "default/b".to_string(),
                resource("b", "https://b.example.com", Some(b"b-key")),
            )]),
            HashMap::from([(
                "default/a".to_string(),
                resource("a", "https://updated.example.com/prefix", Some(b"new-key")),
            )]),
            HashSet::new(),
        );
        assert_eq!(store.count(), 2);
        assert_eq!(
            store.get_by_key("default/a").unwrap().credential_pool.credentials[0].credential,
            "new-key"
        );

        store.partial_update(
            HashMap::new(),
            HashMap::from([(
                "default/a".to_string(),
                resource("a", "https://updated.example.com", None),
            )]),
            HashSet::from(["default/b".to_string()]),
        );
        assert_eq!(store.count(), 0);
    }

    #[test]
    fn full_set_omits_unresolved_and_invalid_resources() {
        let store = AiBackendStore::new();
        let resources = HashMap::from([
            (
                "default/unresolved".to_string(),
                resource("unresolved", "https://ai.example.com", None),
            ),
            (
                "default/invalid".to_string(),
                resource("invalid", "http://ai.example.com", Some(b"key")),
            ),
        ]);
        store.full_set(&resources);
        assert_eq!(store.count(), 0);
    }

    #[test]
    fn compile_backend_keeps_partial_good_credential_pool() {
        let mut configured = resource("provider", "https://ai.example.com", Some(b"usable-key"));
        let spec = Arc::get_mut(&mut configured).unwrap().spec.ai.as_mut().unwrap();
        let mut unresolved = spec.credential_pool.credentials[0].clone();
        unresolved.name = "secondary".to_string();
        unresolved.credential.secret_ref.name = "missing-key".to_string();
        unresolved.credential.secret = None;
        spec.credential_pool.credentials.push(unresolved);

        let backend =
            try_compile_backend("default/provider", &configured).expect("one valid credential keeps backend active");
        assert_eq!(backend.credential_pool.credentials.len(), 1);
        assert_eq!(backend.credential_pool.credentials[0].name, "primary");
        assert_eq!(backend.credential_pool.credentials[0].credential, "usable-key");
    }

    #[test]
    fn compile_backend_preserves_credential_pool_policy_and_limits() {
        let mut configured = resource("provider", "https://ai.example.com", Some(b"usable-key"));
        let credential_pool = &mut Arc::get_mut(&mut configured)
            .unwrap()
            .spec
            .ai
            .as_mut()
            .unwrap()
            .credential_pool;
        credential_pool.redis_ref = Some("default/ai-limits".to_string());
        credential_pool.on_redis_failure = OnRedisFailure::Allow;
        credential_pool.credentials[0].weight = 17;
        credential_pool.credentials[0].limits = Some(AiCredentialLimits {
            requests_per_minute: Some(120),
            tokens_per_minute: Some(50_000),
        });

        let backend = try_compile_backend("default/provider", &configured).expect("valid limited credential pool");
        assert_eq!(backend.credential_pool.redis_ref.as_deref(), Some("default/ai-limits"));
        assert_eq!(backend.credential_pool.on_redis_failure, OnRedisFailure::Allow);
        let credential = &backend.credential_pool.credentials[0];
        assert_eq!(credential.weight, 17);
        assert_eq!(credential.limits.as_ref().unwrap().requests_per_minute, Some(120));
        assert_eq!(credential.limits.as_ref().unwrap().tokens_per_minute, Some(50_000));
    }

    #[test]
    fn snapshot_keeps_one_service_generation() {
        let store = AiBackendStore::new();
        store.partial_update(
            HashMap::from([(
                "default/provider".to_string(),
                resource("provider", "https://old.example.com", Some(b"old-key")),
            )]),
            HashMap::new(),
            HashSet::new(),
        );
        let snapshot = store.snapshot();

        store.partial_update(
            HashMap::new(),
            HashMap::from([(
                "default/provider".to_string(),
                resource("provider", "https://new.example.com", Some(b"new-key")),
            )]),
            HashSet::new(),
        );

        assert_eq!(
            snapshot.get_by_key("default/provider").unwrap().endpoint.domain,
            "old.example.com"
        );
        assert_eq!(
            store.get_by_key("default/provider").unwrap().endpoint.domain,
            "new.example.com"
        );
    }

    #[test]
    fn credential_limiter_survives_unrelated_service_updates() {
        let store = AiBackendStore::new();
        let original = resource("provider", "https://ai.example.com", Some(b"key"));
        store.full_set(&HashMap::from([("default/provider".to_string(), original)]));
        let before = store.get_by_key("default/provider").unwrap();

        let mut updated = resource("provider", "https://ai.example.com", Some(b"key"));
        Arc::get_mut(&mut updated).unwrap().spec.ai.as_mut().unwrap().models[0]
            .aliases
            .push("balanced".to_string());
        store.partial_update(
            HashMap::new(),
            HashMap::from([("default/provider".to_string(), updated)]),
            HashSet::new(),
        );

        let after = store.get_by_key("default/provider").unwrap();
        assert!(!Arc::ptr_eq(&before, &after));
        assert!(Arc::ptr_eq(
            &before.credential_pool.limiter_core,
            &after.credential_pool.limiter_core
        ));
    }

    #[test]
    fn snapshot_multi_get_preserves_order_missing_slots_and_generation() {
        let store = AiBackendStore::new();
        store.partial_update(
            HashMap::from([
                (
                    "default/a".to_string(),
                    resource("a", "https://a-old.example.com", Some(b"a-key")),
                ),
                (
                    "default/b".to_string(),
                    resource("b", "https://b-old.example.com", Some(b"b-key")),
                ),
            ]),
            HashMap::new(),
            HashSet::new(),
        );
        let snapshot = store.snapshot();

        store.partial_update(
            HashMap::new(),
            HashMap::from([(
                "default/a".to_string(),
                resource("a", "https://a-new.example.com", Some(b"a-key")),
            )]),
            HashSet::from(["default/b".to_string()]),
        );

        let resolved = snapshot.get_many_by_key(["default/b", "default/missing", "default/a"]);
        assert_eq!(resolved.len(), 3);
        assert_eq!(resolved[0].as_ref().unwrap().endpoint.domain, "b-old.example.com");
        assert!(resolved[1].is_none());
        assert_eq!(resolved[2].as_ref().unwrap().endpoint.domain, "a-old.example.com");
        assert_eq!(
            store.get_by_key("default/a").unwrap().endpoint.domain,
            "a-new.example.com"
        );
        assert!(store.get_by_key("default/b").is_none());
    }

    #[test]
    fn equivalent_resync_reuses_runtime_service_arc() {
        let store = AiBackendStore::new();
        let first = resource("provider", "https://ai.example.com/v1", Some(b"key"));
        store.full_set(&HashMap::from([("default/provider".to_string(), first)]));
        let before = store.get_by_key("default/provider").unwrap();

        // A trailing slash normalizes to the same compiled path prefix.
        let equivalent = resource("provider", "https://ai.example.com/v1/", Some(b"key"));
        store.full_set(&HashMap::from([("default/provider".to_string(), equivalent)]));
        let after = store.get_by_key("default/provider").unwrap();
        assert!(Arc::ptr_eq(&before, &after));
        assert!(changed_ai_backend_targets(
            &HashMap::from([(
                "default/provider".to_string(),
                Arc::new(RuntimeEdgionBackend::Ai(before)),
            )]),
            &HashMap::from([(
                "default/provider".to_string(),
                Arc::new(RuntimeEdgionBackend::Ai(Arc::clone(&after))),
            )]),
        )
        .is_empty());

        let changed = resource("provider", "https://changed.example.com/v1", Some(b"key"));
        store.full_set(&HashMap::from([("default/provider".to_string(), changed)]));
        let changed = store.get_by_key("default/provider").unwrap();
        assert!(!Arc::ptr_eq(&after, &changed));
        assert_eq!(
            changed_ai_backend_targets(
                &HashMap::from([(
                    "default/provider".to_string(),
                    Arc::new(RuntimeEdgionBackend::Ai(after)),
                )]),
                &HashMap::from([(
                    "default/provider".to_string(),
                    Arc::new(RuntimeEdgionBackend::Ai(changed)),
                )]),
            ),
            vec![PolicyTargetKey {
                target_kind: PolicyTargetKind::EdgionBackend,
                namespace: "default".to_string(),
                name: "provider".to_string(),
            }]
        );
    }

    #[test]
    fn adaptive_target_changes_include_service_additions_and_removals() {
        let resource = resource("provider", "https://ai.example.com/v1", Some(b"key"));
        let backend = Arc::new(compile_backend("default/provider", &resource).expect("backend fixture must compile"));
        let empty = RuntimeMap::new();
        let populated = RuntimeMap::from([("default/provider".to_string(), backend)]);
        let target = PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionBackend,
            namespace: "default".to_string(),
            name: "provider".to_string(),
        };

        assert_eq!(changed_ai_backend_targets(&empty, &populated), vec![target.clone()]);
        assert_eq!(changed_ai_backend_targets(&populated, &empty), vec![target]);
    }

    #[test]
    fn removing_a_service_drops_its_credential_outlier_entry() {
        // The entry is only reachable through a live backend, so leaving one
        // behind is a pure leak — and it is what the integration suite's
        // post-wipe-all residue check catches.
        let store = AiBackendStore::new();
        let key = "default/provider";
        store.full_set(&HashMap::from([(
            key.to_string(),
            resource("provider", "https://ai.example.com", Some(b"key")),
        )]));
        assert_eq!(store.credential_outliers.count(), 1);

        store.full_set(&HashMap::new());
        assert_eq!(
            store.credential_outliers.count(),
            0,
            "credential outlier entries survived their backend: {:?}",
            store.credential_outliers.keys()
        );
    }

    #[test]
    fn an_invalid_update_also_drops_the_entry() {
        // A resource that fails to compile is removed from the runtime map, so
        // its outlier entry must go too — otherwise a permanently broken
        // resource leaks one forever.
        let store = AiBackendStore::new();
        let key = "default/provider";
        store.full_set(&HashMap::from([(
            key.to_string(),
            resource("provider", "https://ai.example.com", Some(b"key")),
        )]));
        assert_eq!(store.credential_outliers.count(), 1);

        // No credential material => compile fails => deactivated.
        store.full_set(&HashMap::from([(
            key.to_string(),
            resource("provider", "https://ai.example.com", None),
        )]));
        assert!(store.get_by_key(key).is_none());
        assert_eq!(store.credential_outliers.count(), 0);
    }

    #[test]
    fn credential_publish_remove_hides_new_capture_but_keeps_old_handle_alive() {
        let store = AiBackendStore::new();
        let key = "default/provider";
        store.full_set(&HashMap::from([(
            key.to_string(),
            resource("provider", "https://ai.example.com", Some(b"key")),
        )]));
        let target = AiBackendStore::target_key(key).unwrap();
        let generation = store
            .credential_outliers
            .current_handle(&target)
            .expect("default generation");
        let captured = generation.capture("primary").expect("published credential");

        store.full_set(&HashMap::new());
        assert!(store.get_by_key(key).is_none());
        // No new capture is possible: the generation is gone entirely rather
        // than lingering with an empty alias set, which is what kept these
        // entries accumulating for the process lifetime.
        assert!(store.credential_outliers.current_handle(&target).is_none());
        // The contract this test exists for: a request that already captured a
        // generation keeps it alive through its own Arc and still sees the
        // ejection state.
        assert!(matches!(
            captured.admit(),
            crate::backends::outlier_detection::EjectionAdmission::Closed
        ));
    }

    #[test]
    fn runtime_identity_prefers_kubernetes_uid_and_uid_change_breaks_equivalence() {
        let store = AiBackendStore::new();
        let mut first = resource("provider", "https://ai.example.com/v1", Some(b"key"));
        Arc::get_mut(&mut first).unwrap().metadata.uid = Some("uid-generation-a".to_string());
        store.full_set(&HashMap::from([("default/provider".to_string(), first)]));
        let before = store.get_by_key("default/provider").unwrap();
        assert_eq!(before.uid, "uid-generation-a");

        let mut recreated = resource("provider", "https://ai.example.com/v1", Some(b"key"));
        Arc::get_mut(&mut recreated).unwrap().metadata.uid = Some("uid-generation-b".to_string());
        store.full_set(&HashMap::from([("default/provider".to_string(), recreated)]));
        let after = store.get_by_key("default/provider").unwrap();
        assert_eq!(after.uid, "uid-generation-b");
        assert!(!Arc::ptr_eq(&before, &after));
    }

    #[test]
    fn endpoint_root_and_ipv6_authority_are_compiled_once() {
        let root = compile_endpoint(&Url::parse("https://example.com/").unwrap()).unwrap();
        assert!(root.path_prefix.is_empty());
        assert_eq!(root.host, "example.com");

        let ipv6 = compile_endpoint(&Url::parse("https://[2001:db8::1]:8443/base/").unwrap()).unwrap();
        assert_eq!(ipv6.domain, "2001:db8::1");
        assert_eq!(ipv6.host, "[2001:db8::1]:8443");
        assert_eq!(ipv6.path_prefix, "/base");
    }

    #[test]
    fn debug_never_exposes_credential() {
        let backend = try_compile_backend(
            "default/provider",
            &resource(
                "provider",
                "https://ai.example.com",
                Some(b"credential-must-not-appear"),
            ),
        )
        .unwrap();
        let debug = format!("{backend:?}");
        assert!(!debug.contains("credential-must-not-appear"));
        assert!(!debug.contains("credential:"));
        assert!(debug.contains("ai.example.com"));
    }
}
