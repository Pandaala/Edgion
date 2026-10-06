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
use edgion_resources::credentials::{AiApiKeyResolver, SecretMaterialResolver};
use edgion_resources::resources::edgion_backend::{
    AiCredentialLimits, AiProvider, EdgionBackend, ParsedAiModelPricing,
};
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
use edgion_resources::resources::edgion_plugins::OnRedisFailure;
use url::{Host, Url};

type RuntimeMap = HashMap<String, Arc<RuntimeAiBackend>>;
type CredentialOutlierMap = HashMap<PolicyTargetKey, Arc<crate::ai::AiCredentialOutlierBackendHandle>>;

static GLOBAL_EDGION_BACKEND_STORE: LazyLock<Arc<EdgionBackendStore>> = LazyLock::new(|| {
    Arc::new(EdgionBackendStore::with_credential_outlier_store(
        crate::ai::get_ai_credential_outlier_store(),
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

struct Snapshot {
    generation: u64,
    backends: RuntimeMap,
    /// Immutable config-free fallback projection for this exact backend
    /// generation. The mutable credential store remains the single cold-path
    /// owner; request planning never consults it directly.
    credential_outliers: CredentialOutlierMap,
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
    pub fn generation(&self) -> u64 {
        self.inner.generation
    }

    pub fn get_by_key(&self, key: &str) -> Option<Arc<RuntimeAiBackend>> {
        self.inner.backends.get(key).cloned()
    }

    /// Resolve several candidate keys from this one captured generation.
    /// Input order and missing slots are preserved for route-weight alignment.
    pub fn get_many_by_key<'a>(&self, keys: impl IntoIterator<Item = &'a str>) -> Vec<Option<Arc<RuntimeAiBackend>>> {
        keys.into_iter().map(|key| self.get_by_key(key)).collect()
    }

    pub fn credential_outlier_handle(
        &self,
        target: &PolicyTargetKey,
    ) -> Option<Arc<crate::ai::AiCredentialOutlierBackendHandle>> {
        self.inner.credential_outliers.get(target).cloned()
    }
}

/// Atomic store of all activatable AI backends.
pub struct EdgionBackendStore {
    /// Production writer invariant: this handler is owned by exactly one
    /// ConfigClient cache, whose list/watch callbacks are serialized under the
    /// cache-data write lock. Therefore partial updates use load-clone-store
    /// without losing writes. A second production writer would require `rcu`
    /// or external serialization.
    snapshot: ArcSwap<Snapshot>,
    credential_outliers: Arc<crate::ai::AiCredentialOutlierStore>,
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
        Self::with_credential_outlier_store(Arc::new(crate::ai::AiCredentialOutlierStore::new()))
    }

    fn with_credential_outlier_store(credential_outliers: Arc<crate::ai::AiCredentialOutlierStore>) -> Self {
        Self {
            snapshot: ArcSwap::from_pointee(Snapshot {
                generation: 0,
                backends: HashMap::new(),
                credential_outliers: HashMap::new(),
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

    /// Publish one backend's credential role and return the config-free
    /// fallback Arc for the next immutable backend snapshot.
    fn publish_credentials(
        &self,
        key: &str,
        backend: Option<&RuntimeAiBackend>,
    ) -> Option<(PolicyTargetKey, Arc<crate::ai::AiCredentialOutlierBackendHandle>)> {
        let target = Self::target_key(key)?;
        let Some(backend) = backend else {
            self.credential_outliers.remove_credentials(&target);
            return None;
        };
        let aliases = backend
            .credential_pool
            .credentials
            .iter()
            .map(|credential| credential.name.clone());
        let fallback = self.credential_outliers.set_credentials(target.clone(), aliases)?;
        Some((target, fallback))
    }

    /// Reconcile the one mutable credential entry per target, then derive the
    /// immutable fallback projection published with `next`.
    fn build_credential_outlier_projection(&self, current: &RuntimeMap, next: &RuntimeMap) -> CredentialOutlierMap {
        for key in current.keys() {
            if !next.contains_key(key) {
                self.publish_credentials(key, None);
            }
        }
        next.iter()
            .filter_map(|(key, backend)| self.publish_credentials(key, Some(backend)))
            .collect()
    }

    /// One lock-free snapshot load plus one hash-table lookup.
    pub fn get_by_key(&self, key: &str) -> Option<Arc<RuntimeAiBackend>> {
        self.snapshot.load().backends.get(key).cloned()
    }

    /// Capture one immutable generation for a complete request/attempt plan.
    pub fn snapshot(&self) -> AiBackendSnapshot {
        AiBackendSnapshot {
            inner: self.snapshot.load_full(),
        }
    }

    /// Number of activatable entries. Invalid or unresolved resources are absent.
    pub fn count(&self) -> usize {
        self.snapshot.load().backends.len()
    }

    pub fn keys(&self) -> Vec<String> {
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
        let credential_outliers = self.build_credential_outlier_projection(&current.backends, &backends);
        self.snapshot.store(Arc::new(Snapshot {
            generation: current
                .generation
                .checked_add(1)
                .expect("EdgionBackend snapshot generation overflow"),
            backends,
            credential_outliers,
        }));
        if std::ptr::eq(self, GLOBAL_EDGION_BACKEND_STORE.as_ref()) {
            let adaptive_store = crate::ai::get_adaptive_telemetry_store();
            for target in &adaptive_targets {
                adaptive_store.remove_target(target);
            }
            crate::ai::reconcile_adaptive_targets(&adaptive_targets);
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
        let credential_outliers = self.build_credential_outlier_projection(&current.backends, &backends);
        self.snapshot.store(Arc::new(Snapshot {
            generation: current
                .generation
                .checked_add(1)
                .expect("EdgionBackend snapshot generation overflow"),
            backends,
            credential_outliers,
        }));
        if std::ptr::eq(self, GLOBAL_EDGION_BACKEND_STORE.as_ref()) {
            let adaptive_store = crate::ai::get_adaptive_telemetry_store();
            for target in &adaptive_targets {
                adaptive_store.remove_target(target);
            }
            crate::ai::reconcile_adaptive_targets(&adaptive_targets);
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
            (Some(previous), Some(updated)) => !Arc::ptr_eq(previous, updated),
            (Some(_), None) | (None, Some(_)) => true,
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
include!("../../../tests/support/ai/runtime/backend_store_test/424_static_item.rs");

#[cfg(test)]
include!("../../../tests/support/ai/runtime/backend_store_test/431_struct_item.rs");

#[cfg(test)]
include!("../../../tests/support/ai/runtime/backend_store_test/436_impl_item.rs");

#[cfg(test)]
include!("../../../tests/support/ai/runtime/backend_store_test/443_function_item.rs");

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

fn compile_backend(key: &str, resource: &EdgionBackend) -> Option<RuntimeAiBackend> {
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
    previous: Option<&Arc<RuntimeAiBackend>>,
) -> Option<Arc<RuntimeAiBackend>> {
    let mut compiled = compile_backend(key, resource)?;
    if let Some(previous) = previous.filter(|previous| previous.as_ref() == &compiled) {
        return Some(previous.clone());
    }
    if let Some(previous_ai) = previous.map(|backend| backend.as_ref()) {
        if previous_ai.credential_pool == compiled.credential_pool {
            // Preserve request-shared limiter state for an otherwise changed AI
            // entry while retaining the existing limiter state.
            compiled.credential_pool.limiter_core = Arc::clone(&previous_ai.credential_pool.limiter_core);
        }
    }
    Some(Arc::new(compiled))
}

fn try_compile_edgion_backend(key: &str, resource: &EdgionBackend) -> Result<RuntimeAiBackend, &'static str> {
    resource.spec.validate().map_err(|_| "invalid configuration")?;
    try_compile_backend(key, resource)
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

fn try_compile_backend(key: &str, resource: &EdgionBackend) -> Result<RuntimeAiBackend, &'static str> {
    let ai = &resource.spec.ai;

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

#[cfg(test)]
#[path = "../../../tests/unit/ai/runtime/backend_store_test.rs"]
mod tests;
