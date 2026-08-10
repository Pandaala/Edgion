//! Lock-free runtime index for [`EdgionAIService`] resources.
//!
//! The conf-sync path validates and compiles resources into a single immutable
//! snapshot. Request processing captures one ArcSwap generation and performs
//! bounded hash lookups within it; it never reads the typed ConfigClient cache
//! or reparses endpoint, model, or credential data.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, LazyLock};

use arc_swap::ArcSwap;
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::credentials::{AiApiKeyResolver, SecretMaterialResolver};
use edgion_resources::resources::edgion_ai_service::{
    AiCredentialLimits, AiProvider, EdgionAIService, ParsedAiModelPricing,
};
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
use edgion_resources::resources::edgion_plugins::OnRedisFailure;
use url::{Host, Url};

type RuntimeMap = HashMap<String, Arc<RuntimeAiService>>;

static GLOBAL_AI_SERVICE_STORE: LazyLock<Arc<AiServiceStore>> = LazyLock::new(|| {
    Arc::new(AiServiceStore::with_credential_outlier_store(
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

/// Canonical model selected by either its canonical name or an alias.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedModel {
    pub canonical_name: String,
    pub pricing: Option<ParsedAiModelPricing>,
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

/// Request-ready AI service configuration.
///
/// The original resource is intentionally not retained: ClientCache owns the
/// transport object, while this value contains only cold-compiled request data.
#[derive(PartialEq, Eq)]
pub struct RuntimeAiService {
    /// Stable identity for credential Redis keys and runtime state. Kubernetes
    /// resources use metadata.uid; UID-less config centers use the typed key.
    pub uid: String,
    pub provider: AiProvider,
    pub endpoint: RuntimeAiEndpoint,
    pub streaming: bool,
    pub max_output_tokens: Option<u32>,
    pub credential_pool: RuntimeAiCredentialPool,
    pub models: HashMap<String, ResolvedModel>,
}

impl fmt::Debug for RuntimeAiService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeAiService")
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
    services: RuntimeMap,
    credential_outliers: Arc<super::AiCredentialOutlierStore>,
}

/// Request-stable view of all activatable AI services.
///
/// A route with multiple AI backends captures this once, then resolves every
/// candidate from the same generation without repeated ArcSwap loads.
#[derive(Clone)]
pub struct AiServiceSnapshot {
    inner: Arc<Snapshot>,
}

impl AiServiceSnapshot {
    pub fn get_by_key(&self, key: &str) -> Option<Arc<RuntimeAiService>> {
        self.inner.services.get(key).cloned()
    }

    /// Resolve several candidate keys from this one captured generation.
    /// Input order and missing slots are preserved for route-weight alignment.
    pub fn get_many_by_key<'a>(&self, keys: impl IntoIterator<Item = &'a str>) -> Vec<Option<Arc<RuntimeAiService>>> {
        keys.into_iter().map(|key| self.get_by_key(key)).collect()
    }

    pub fn credential_outlier_handle(
        &self,
        target: &PolicyTargetKey,
    ) -> Option<Arc<super::AiCredentialOutlierServiceHandle>> {
        self.inner.credential_outliers.current_handle(target)
    }
}

/// Atomic store of activatable AI services.
pub struct AiServiceStore {
    /// Production writer invariant: this handler is owned by exactly one
    /// ConfigClient cache, whose list/watch callbacks are serialized under the
    /// cache-data write lock. Therefore partial updates use load-clone-store
    /// without losing writes. A second production writer would require `rcu`
    /// or external serialization.
    snapshot: ArcSwap<Snapshot>,
    credential_outliers: Arc<super::AiCredentialOutlierStore>,
}

impl Default for AiServiceStore {
    fn default() -> Self {
        Self::new()
    }
}

impl AiServiceStore {
    pub fn new() -> Self {
        Self::with_credential_outlier_store(Arc::new(super::AiCredentialOutlierStore::new()))
    }

    fn with_credential_outlier_store(credential_outliers: Arc<super::AiCredentialOutlierStore>) -> Self {
        Self {
            snapshot: ArcSwap::from_pointee(Snapshot {
                services: HashMap::new(),
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
            target_kind: PolicyTargetKind::EdgionAIService,
            namespace: namespace.to_string(),
            name: name.to_string(),
        })
    }

    /// Mirror the service's credential pool into the outlier store.
    ///
    /// `None` means the service is no longer activatable — removed, or an
    /// update that failed to compile. The entry is dropped rather than emptied:
    /// it is only reachable through a live service, so an emptied one is
    /// unreachable state that accumulates for the process lifetime. Requests
    /// already holding a captured generation keep it alive through their own
    /// `Arc`, which is what preserves in-flight ejection state.
    fn publish_credentials(&self, key: &str, service: Option<&RuntimeAiService>) {
        let Some(target) = Self::target_key(key) else {
            return;
        };
        let Some(service) = service else {
            self.credential_outliers.remove_service(&target);
            return;
        };
        let aliases = service
            .credential_pool
            .credentials
            .iter()
            .map(|credential| credential.name.clone());
        self.credential_outliers.set_credentials(target, aliases);
    }

    /// One lock-free snapshot load plus one hash-table lookup.
    pub fn get_by_key(&self, key: &str) -> Option<Arc<RuntimeAiService>> {
        self.snapshot.load().services.get(key).cloned()
    }

    /// Capture one immutable generation for a complete request/attempt plan.
    pub fn snapshot(&self) -> AiServiceSnapshot {
        AiServiceSnapshot {
            inner: self.snapshot.load_full(),
        }
    }

    /// Number of activatable entries. Invalid or unresolved resources are absent.
    pub fn count(&self) -> usize {
        self.snapshot.load().services.len()
    }

    pub fn keys(&self) -> Vec<String> {
        self.snapshot.load().services.keys().cloned().collect()
    }

    fn replace_all(&self, resources: &HashMap<String, Arc<EdgionAIService>>) {
        let current = self.snapshot.load();
        let services: RuntimeMap = resources
            .iter()
            .filter_map(|(key, resource)| {
                compile_service_entry(key, resource, current.services.get(key)).map(|service| (key.clone(), service))
            })
            .collect();
        let adaptive_targets = changed_service_targets(&current.services, &services);
        for key in current.services.keys() {
            if !services.contains_key(key) {
                self.publish_credentials(key, None);
            }
        }
        for (key, service) in &services {
            self.publish_credentials(key, Some(service));
        }
        self.snapshot.store(Arc::new(Snapshot {
            services,
            credential_outliers: Arc::clone(&self.credential_outliers),
        }));
        if std::ptr::eq(self, GLOBAL_AI_SERVICE_STORE.as_ref()) {
            let adaptive_store = super::get_adaptive_telemetry_store();
            for target in &adaptive_targets {
                adaptive_store.remove_target(target);
            }
            super::reconcile_adaptive_targets(&adaptive_targets);
        }
    }

    fn update(&self, add_or_update: HashMap<String, Arc<EdgionAIService>>, remove: &HashSet<String>) {
        let current = self.snapshot.load();
        let mut services = current.services.clone();
        for key in remove {
            services.remove(key);
            self.publish_credentials(key, None);
        }
        for (key, resource) in add_or_update {
            match compile_service_entry(&key, &resource, services.get(&key)) {
                Some(service) => {
                    self.publish_credentials(&key, Some(&service));
                    services.insert(key, service);
                }
                None => {
                    // An invalid update must deactivate the previous generation.
                    services.remove(&key);
                    self.publish_credentials(&key, None);
                }
            }
        }
        let adaptive_targets = changed_service_targets(&current.services, &services);
        self.snapshot.store(Arc::new(Snapshot {
            services,
            credential_outliers: Arc::clone(&self.credential_outliers),
        }));
        if std::ptr::eq(self, GLOBAL_AI_SERVICE_STORE.as_ref()) {
            let adaptive_store = super::get_adaptive_telemetry_store();
            for target in &adaptive_targets {
                adaptive_store.remove_target(target);
            }
            super::reconcile_adaptive_targets(&adaptive_targets);
        }
    }
}

fn changed_service_targets(current: &RuntimeMap, next: &RuntimeMap) -> Vec<PolicyTargetKey> {
    current
        .keys()
        .chain(next.keys())
        .collect::<HashSet<_>>()
        .into_iter()
        .filter(|key| match (current.get(*key), next.get(*key)) {
            (Some(previous), Some(updated)) => !Arc::ptr_eq(previous, updated),
            (None, None) => false,
            _ => true,
        })
        .filter_map(|key| AiServiceStore::target_key(key))
        .collect()
}

pub fn get_global_ai_service_store() -> Arc<AiServiceStore> {
    GLOBAL_AI_SERVICE_STORE.clone()
}

/// Borrow the process-global store without incrementing the outer Arc.
///
/// Multi-candidate request-path consumers should use this accessor, capture one
/// [`AiServiceSnapshot`], then resolve every candidate from that view.
#[inline]
pub fn global_ai_service_store() -> &'static AiServiceStore {
    GLOBAL_AI_SERVICE_STORE.as_ref()
}

pub fn create_ai_service_handler() -> Box<dyn ConfHandler<EdgionAIService> + Send + Sync> {
    Box::new(get_global_ai_service_store())
}

pub fn count_ai_services() -> usize {
    GLOBAL_AI_SERVICE_STORE.count()
}

#[cfg(test)]
static GLOBAL_AI_SERVICE_TEST_LOCK: LazyLock<parking_lot::Mutex<()>> = LazyLock::new(|| parking_lot::Mutex::new(()));

/// Exclusive guard for tests that mutate the process-global AI store.
///
/// Drop always clears the store, including during unwinding after a failed
/// assertion, so one test cannot leak resources into another.
#[cfg(test)]
pub struct GlobalAiServiceTestGuard {
    _lock: parking_lot::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for GlobalAiServiceTestGuard {
    fn drop(&mut self) {
        GLOBAL_AI_SERVICE_STORE.replace_all(&HashMap::new());
    }
}

#[cfg(test)]
pub fn lock_global_ai_service_store_for_test() -> GlobalAiServiceTestGuard {
    let lock = GLOBAL_AI_SERVICE_TEST_LOCK.lock();
    GLOBAL_AI_SERVICE_STORE.replace_all(&HashMap::new());
    GlobalAiServiceTestGuard { _lock: lock }
}

impl ConfHandler<EdgionAIService> for AiServiceStore {
    fn full_set(&self, data: &HashMap<String, Arc<EdgionAIService>>) {
        tracing::info!(component = "ai_service_store", count = data.len(), "full set");
        self.replace_all(data);
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<EdgionAIService>>,
        update: HashMap<String, Arc<EdgionAIService>>,
        remove: HashSet<String>,
    ) {
        tracing::info!(
            component = "ai_service_store",
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

fn compile_service(key: &str, resource: &EdgionAIService) -> Option<RuntimeAiService> {
    match try_compile_service(key, resource) {
        Ok(service) => Some(service),
        Err(reason) => {
            tracing::warn!(
                component = "ai_service_store",
                key = key,
                reason = reason,
                "AI service omitted from activatable runtime index"
            );
            None
        }
    }
}

/// Compile a generation, retaining the prior Arc when normalized runtime data
/// is equivalent. This makes no-op resync behavior explicit and avoids future
/// per-service state churn while captured request snapshots remain immutable.
fn compile_service_entry(
    key: &str,
    resource: &EdgionAIService,
    previous: Option<&Arc<RuntimeAiService>>,
) -> Option<Arc<RuntimeAiService>> {
    let mut compiled = compile_service(key, resource)?;
    if let Some(previous) = previous.filter(|previous| previous.as_ref() == &compiled) {
        return Some(previous.clone());
    }
    if let Some(previous) = previous.filter(|previous| previous.credential_pool == compiled.credential_pool) {
        compiled.credential_pool.limiter_core = Arc::clone(&previous.credential_pool.limiter_core);
    }
    Some(Arc::new(compiled))
}

fn try_compile_service(key: &str, resource: &EdgionAIService) -> Result<RuntimeAiService, &'static str> {
    resource.spec.validate().map_err(|_| "invalid configuration")?;

    let uid = match resource.metadata.uid.as_deref().filter(|uid| !uid.is_empty()) {
        Some(uid) => uid.to_string(),
        None => {
            let (namespace, name) = key.split_once('/').ok_or("invalid resource key")?;
            if namespace.is_empty() || name.is_empty() {
                return Err("invalid resource key");
            }
            PolicyTargetKey {
                target_kind: PolicyTargetKind::EdgionAIService,
                namespace: namespace.to_string(),
                name: name.to_string(),
            }
            .to_string()
        }
    };

    let endpoint = resource.spec.effective_endpoint().map_err(|_| "invalid endpoint")?;
    let endpoint = compile_endpoint(&endpoint).ok_or("invalid endpoint")?;
    let resolver = AiApiKeyResolver;
    let credentials = resource
        .spec
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
        redis_ref: resource.spec.credential_pool.redis_ref.clone(),
        on_redis_failure: resource.spec.credential_pool.on_redis_failure.clone(),
        credentials,
        limiter_core: Arc::new(crate::plugins::common::redis_rate_limiter::RedisRateLimiter::new()),
    };

    let canonical_pricing: HashMap<&str, Option<ParsedAiModelPricing>> = resource
        .spec
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
    let models = resource
        .spec
        .model_alias_table()
        .map_err(|_| "invalid model catalog")?
        .into_iter()
        .map(|(name, canonical_name)| {
            let pricing = canonical_pricing.get(canonical_name.as_str()).copied().flatten();
            (
                name,
                ResolvedModel {
                    canonical_name,
                    pricing,
                },
            )
        })
        .collect();

    Ok(RuntimeAiService {
        uid,
        provider: resource.spec.provider,
        endpoint,
        streaming: resource.spec.defaults.streaming,
        max_output_tokens: resource.spec.defaults.max_output_tokens,
        credential_pool,
        models,
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
mod tests {
    use super::*;
    use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
    use edgion_resources::resources::edgion_ai_service::{
        AiCredential, AiCredentialPool, AiModel, AiModelPricing, AiServiceDefaults, EdgionAIServiceSpec,
    };
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    fn resource(name: &str, endpoint: &str, credential: Option<&[u8]>) -> Arc<EdgionAIService> {
        let secret = credential.map(|value| Secret {
            data: Some(BTreeMap::from([("api-key".to_string(), ByteString(value.to_vec()))])),
            ..Default::default()
        });
        let spec = EdgionAIServiceSpec {
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
                pricing: None,
            }],
            defaults: AiServiceDefaults {
                streaming: true,
                max_output_tokens: Some(2048),
            },
        };
        let mut resource = EdgionAIService::new(name, spec);
        resource.metadata.namespace = Some("default".to_string());
        Arc::new(resource)
    }

    #[test]
    fn starts_empty() {
        let store = AiServiceStore::new();
        assert_eq!(store.count(), 0);
        assert!(store.keys().is_empty());
        assert!(store.get_by_key("default/missing").is_none());
    }

    #[test]
    fn full_set_atomically_publishes_compiled_service() {
        let store = AiServiceStore::new();
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
        let service = store.get_by_key("default/provider").expect("compiled service");
        assert_eq!(service.uid, "edgion.io/EdgionAIService/default/provider");
        assert_eq!(service.endpoint.domain, "ai.example.com");
        assert_eq!(service.endpoint.port, 8443);
        assert!(service.endpoint.use_tls);
        assert_eq!(service.endpoint.sni, "ai.example.com");
        assert_eq!(service.endpoint.host, "ai.example.com:8443");
        assert_eq!(service.endpoint.path_prefix, "/openai/v1");
        assert!(service.streaming);
        assert_eq!(service.max_output_tokens, Some(2048));
        assert_eq!(service.credential_pool.credentials.len(), 1);
        assert_eq!(service.credential_pool.credentials[0].name, "primary");
        assert_eq!(service.credential_pool.credentials[0].credential, "secret-value");
        assert_eq!(service.models["model-a"].canonical_name, "model-a");
        assert_eq!(service.models["fast"].canonical_name, "model-a");
    }

    #[test]
    fn model_pricing_is_compiled_once_and_shared_by_aliases() {
        let mut configured = resource("provider", "https://ai.example.com", Some(b"secret-value"));
        Arc::get_mut(&mut configured).unwrap().spec.models[0].pricing = Some(AiModelPricing {
            input_per_million_usd: "15".to_string(),
            output_per_million_usd: "75".to_string(),
            cache_read_input_per_million_usd: Some("1.5".to_string()),
            cache_write_input_per_million_usd: None,
        });
        let service = try_compile_service("default/provider", &configured).unwrap();
        let canonical = service.models["model-a"].pricing.unwrap();
        let alias = service.models["fast"].pricing.unwrap();
        assert_eq!(canonical, alias);
        assert_eq!(canonical.input.get(), 15_000_000);
        assert_eq!(canonical.cache_read_input.get(), 1_500_000);
        assert_eq!(canonical.cache_write_input, canonical.input);
    }

    #[test]
    fn partial_update_adds_updates_removes_and_deactivates_invalid_generation() {
        let store = AiServiceStore::new();
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
        let store = AiServiceStore::new();
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
    fn compile_service_keeps_partial_good_credential_pool() {
        let mut configured = resource("provider", "https://ai.example.com", Some(b"usable-key"));
        let spec = &mut Arc::get_mut(&mut configured).unwrap().spec;
        let mut unresolved = spec.credential_pool.credentials[0].clone();
        unresolved.name = "secondary".to_string();
        unresolved.credential.secret_ref.name = "missing-key".to_string();
        unresolved.credential.secret = None;
        spec.credential_pool.credentials.push(unresolved);

        let service =
            try_compile_service("default/provider", &configured).expect("one valid credential keeps service active");
        assert_eq!(service.credential_pool.credentials.len(), 1);
        assert_eq!(service.credential_pool.credentials[0].name, "primary");
        assert_eq!(service.credential_pool.credentials[0].credential, "usable-key");
    }

    #[test]
    fn compile_service_preserves_credential_pool_policy_and_limits() {
        let mut configured = resource("provider", "https://ai.example.com", Some(b"usable-key"));
        let credential_pool = &mut Arc::get_mut(&mut configured).unwrap().spec.credential_pool;
        credential_pool.redis_ref = Some("default/ai-limits".to_string());
        credential_pool.on_redis_failure = OnRedisFailure::Allow;
        credential_pool.credentials[0].weight = 17;
        credential_pool.credentials[0].limits = Some(AiCredentialLimits {
            requests_per_minute: Some(120),
            tokens_per_minute: Some(50_000),
        });

        let service = try_compile_service("default/provider", &configured).expect("valid limited credential pool");
        assert_eq!(service.credential_pool.redis_ref.as_deref(), Some("default/ai-limits"));
        assert_eq!(service.credential_pool.on_redis_failure, OnRedisFailure::Allow);
        let credential = &service.credential_pool.credentials[0];
        assert_eq!(credential.weight, 17);
        assert_eq!(credential.limits.as_ref().unwrap().requests_per_minute, Some(120));
        assert_eq!(credential.limits.as_ref().unwrap().tokens_per_minute, Some(50_000));
    }

    #[test]
    fn snapshot_keeps_one_service_generation() {
        let store = AiServiceStore::new();
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
        let store = AiServiceStore::new();
        let original = resource("provider", "https://ai.example.com", Some(b"key"));
        store.full_set(&HashMap::from([("default/provider".to_string(), original)]));
        let before = store.get_by_key("default/provider").unwrap();

        let mut updated = resource("provider", "https://ai.example.com", Some(b"key"));
        Arc::get_mut(&mut updated).unwrap().spec.models[0]
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
        let store = AiServiceStore::new();
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
        let store = AiServiceStore::new();
        let first = resource("provider", "https://ai.example.com/v1", Some(b"key"));
        store.full_set(&HashMap::from([("default/provider".to_string(), first)]));
        let before = store.get_by_key("default/provider").unwrap();

        // A trailing slash normalizes to the same compiled path prefix.
        let equivalent = resource("provider", "https://ai.example.com/v1/", Some(b"key"));
        store.full_set(&HashMap::from([("default/provider".to_string(), equivalent)]));
        let after = store.get_by_key("default/provider").unwrap();
        assert!(Arc::ptr_eq(&before, &after));
        assert!(changed_service_targets(
            &HashMap::from([("default/provider".to_string(), before)]),
            &HashMap::from([("default/provider".to_string(), Arc::clone(&after))]),
        )
        .is_empty());

        let changed = resource("provider", "https://changed.example.com/v1", Some(b"key"));
        store.full_set(&HashMap::from([("default/provider".to_string(), changed)]));
        let changed = store.get_by_key("default/provider").unwrap();
        assert!(!Arc::ptr_eq(&after, &changed));
        assert_eq!(
            changed_service_targets(
                &HashMap::from([("default/provider".to_string(), after)]),
                &HashMap::from([("default/provider".to_string(), changed)]),
            ),
            vec![PolicyTargetKey {
                target_kind: PolicyTargetKind::EdgionAIService,
                namespace: "default".to_string(),
                name: "provider".to_string(),
            }]
        );
    }

    #[test]
    fn adaptive_target_changes_include_service_additions_and_removals() {
        let resource = resource("provider", "https://ai.example.com/v1", Some(b"key"));
        let service = Arc::new(compile_service("default/provider", &resource).expect("service fixture must compile"));
        let empty = RuntimeMap::new();
        let populated = RuntimeMap::from([("default/provider".to_string(), service)]);
        let target = PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionAIService,
            namespace: "default".to_string(),
            name: "provider".to_string(),
        };

        assert_eq!(changed_service_targets(&empty, &populated), vec![target.clone()]);
        assert_eq!(changed_service_targets(&populated, &empty), vec![target]);
    }

    #[test]
    fn removing_a_service_drops_its_credential_outlier_entry() {
        // The entry is only reachable through a live service, so leaving one
        // behind is a pure leak — and it is what the integration suite's
        // post-wipe-all residue check catches.
        let store = AiServiceStore::new();
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
            "credential outlier entries survived their service: {:?}",
            store.credential_outliers.keys()
        );
    }

    #[test]
    fn an_invalid_update_also_drops_the_entry() {
        // A resource that fails to compile is removed from the runtime map, so
        // its outlier entry must go too — otherwise a permanently broken
        // resource leaks one forever.
        let store = AiServiceStore::new();
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
        let store = AiServiceStore::new();
        let key = "default/provider";
        store.full_set(&HashMap::from([(
            key.to_string(),
            resource("provider", "https://ai.example.com", Some(b"key")),
        )]));
        let target = AiServiceStore::target_key(key).unwrap();
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
        let store = AiServiceStore::new();
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
        let service = try_compile_service(
            "default/provider",
            &resource(
                "provider",
                "https://ai.example.com",
                Some(b"credential-must-not-appear"),
            ),
        )
        .unwrap();
        let debug = format!("{service:?}");
        assert!(!debug.contains("credential-must-not-appear"));
        assert!(!debug.contains("credential:"));
        assert!(debug.contains("ai.example.com"));
    }
}
