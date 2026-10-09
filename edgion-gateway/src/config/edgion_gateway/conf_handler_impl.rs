use crate::link_sys::providers::webhook::manager::configure_webhook_global_max;
use crate::link_sys::runtime::store::configure_link_sys_max_instances_per_kind;
use crate::request_body::ResolvedRequestBodyConfig;
use crate::routes::http::proxy_http::preflight_handler::ResolvedPreflightPolicy;
use crate::routes::http::proxy_http::ParsedTimeouts;
use crate::util::RealIpExtractor;
use arc_swap::ArcSwap;
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_common::dns::resolver_select::{candidate_from_egc_default, ResolverCandidate};
use edgion_resources::duration::parse_duration;
use edgion_resources::prelude_resources::EdgionGatewayConfig;
use edgion_resources::resources::edgion_gateway_config::{resolve_tcp_connect_timeout, resolve_tcp_idle_timeout};
use kube::ResourceExt;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};
use std::time::Duration;

/// Connection-scoped L4 timeout policy resolved from an EdgionGatewayConfig.
///
/// A new connection copies this value from the current immutable configuration
/// snapshot. In-flight connections keep their copy across retries and relay.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedL4Timeouts {
    pub connect_timeout: Option<Duration>,
    pub idle_timeout: Option<Duration>,
}

impl ResolvedL4Timeouts {
    fn resolve(config: &EdgionGatewayConfig) -> Self {
        let tcp_timeout = config.spec.tcp_timeout.as_ref();
        Self {
            connect_timeout: resolve_tcp_connect_timeout(tcp_timeout),
            idle_timeout: resolve_tcp_idle_timeout(tcp_timeout),
        }
    }
}

impl Default for ResolvedL4Timeouts {
    fn default() -> Self {
        Self {
            connect_timeout: resolve_tcp_connect_timeout(None),
            idle_timeout: resolve_tcp_idle_timeout(None),
        }
    }
}

/// Runtime configuration resolved from one EdgionGatewayConfig generation.
///
/// A new request clones this value from the current immutable configuration
/// snapshot. All fields therefore come from the same publication epoch, while
/// in-flight requests retain their captured values across hot reloads.
#[derive(Clone)]
pub struct ResolvedGatewayConfig {
    pub waf_expensive_checks: bool,
    pub request_body: ResolvedRequestBodyConfig,
    pub remote_ip_header: http::HeaderName,
    pub(crate) preflight_policy: ResolvedPreflightPolicy,
    pub x_forwarded_for_limit: Option<usize>,
    pub require_sni_host_match: bool,
    pub allow_loopback_upstream: bool,
    pub fallback_sni: Option<String>,
    pub tls_proxy_log_record: bool,
    pub parsed_timeouts: ParsedTimeouts,
    pub retry_attempts: u32,
    pub real_ip_extractor: Option<Arc<RealIpExtractor>>,
    pub enable_compression: bool,
    pub downstream_keepalive_request_limit: u32,
    pub l4_timeouts: ResolvedL4Timeouts,
}

impl std::fmt::Debug for ResolvedGatewayConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Do not expose configured hostnames or trusted-proxy topology.
        f.debug_struct("ResolvedGatewayConfig")
            .field("request_body", &self.request_body)
            .field("parsed_timeouts", &self.parsed_timeouts)
            .field("has_real_ip_extractor", &self.real_ip_extractor.is_some())
            .finish_non_exhaustive()
    }
}

impl ResolvedGatewayConfig {
    /// Compile runtime policy on the configuration publication path only.
    pub fn resolve(config: &EdgionGatewayConfig) -> Self {
        let errors = config.spec.validate_forwarded_headers();
        let remote_ip_header = if errors.is_empty() {
            config
                .spec
                .forwarded_headers
                .remote_ip_header
                .parse::<http::HeaderName>()
                .unwrap_or_else(|_| default_remote_ip_header())
        } else {
            tracing::error!(
                config = %config.name_any(),
                errors = ?errors,
                "Ignoring invalid forwardedHeaders configuration received by the gateway"
            );
            default_remote_ip_header()
        };

        let real_ip_errors = config.spec.validate_real_ip();
        let real_ip_extractor = if !real_ip_errors.is_empty() {
            tracing::error!(
                config = %config.name_any(),
                errors = ?real_ip_errors,
                "Ignoring invalid realIp configuration received by the gateway, real IP extraction disabled"
            );
            None
        } else {
            config.spec.real_ip.as_ref().and_then(|real_ip| {
                match RealIpExtractor::new(
                    &real_ip.trusted_ips,
                    real_ip.real_ip_header.clone(),
                    real_ip.recursive,
                    real_ip.max_trusted_hops,
                ) {
                    Ok(extractor) => Some(Arc::new(extractor)),
                    Err(error) => {
                        tracing::warn!(config = %config.name_any(), error = ?error,
                            "Failed to build RealIpExtractor, real IP extraction disabled");
                        None
                    }
                }
            })
        };
        let security = config.spec.security_protect.as_ref();
        let retry_attempts = if config.spec.validate_retry().is_empty() {
            config.spec.retry.attempts
        } else {
            let fallback = edgion_resources::resources::edgion_gateway_config::RetryConfig::default().attempts;
            tracing::error!(
                config = %config.name_any(),
                configured_attempts = config.spec.retry.attempts,
                fallback_attempts = fallback,
                "Invalid retry configuration received by the gateway, using defaults"
            );
            fallback
        };
        Self {
            request_body: ResolvedRequestBodyConfig::resolve(&config.spec.request_body),
            waf_expensive_checks: config.spec.waf.expensive_checks,
            remote_ip_header,
            preflight_policy: ResolvedPreflightPolicy::resolve(config.spec.preflight_policy.as_ref()),
            x_forwarded_for_limit: security.map(|s| s.x_forwarded_for_limit),
            require_sni_host_match: security.is_none_or(|s| s.require_sni_host_match),
            allow_loopback_upstream: security.is_some_and(|s| s.allow_loopback_upstream),
            fallback_sni: security.and_then(|s| s.fallback_sni.clone()),
            tls_proxy_log_record: security.is_none_or(|s| s.tls_proxy_log_record),
            parsed_timeouts: ParsedTimeouts::from_config(config),
            retry_attempts,
            real_ip_extractor,
            enable_compression: config.spec.server.as_ref().is_some_and(|s| s.enable_compression),
            downstream_keepalive_request_limit: config
                .spec
                .server
                .as_ref()
                .map(|s| s.downstream_keepalive_request_limit)
                .unwrap_or(10_000),
            l4_timeouts: ResolvedL4Timeouts::resolve(config),
        }
    }
}

impl Default for ResolvedGatewayConfig {
    fn default() -> Self {
        Self {
            request_body: ResolvedRequestBodyConfig::default(),
            waf_expensive_checks: false,
            remote_ip_header: default_remote_ip_header(),
            preflight_policy: ResolvedPreflightPolicy::default(),
            x_forwarded_for_limit: None,
            require_sni_host_match: true,
            allow_loopback_upstream: false,
            fallback_sni: None,
            tls_proxy_log_record: true,
            parsed_timeouts: ParsedTimeouts::default(),
            retry_attempts: edgion_resources::resources::edgion_gateway_config::EdgionGatewayConfigSpec::default()
                .retry
                .attempts,
            real_ip_extractor: None,
            enable_compression: false,
            downstream_keepalive_request_limit: 10_000,
            l4_timeouts: ResolvedL4Timeouts::default(),
        }
    }
}

fn default_remote_ip_header() -> http::HeaderName {
    http::HeaderName::from_static("x-real-ip")
}

/// Request-path-ready data-plane DNS default resolved from an EdgionGatewayConfig.
///
/// The canonical resolver candidate and cache TTL are derived while publishing
/// the immutable configuration snapshot. Consumers only clone the containing
/// `Arc`; httpdns LinkSys references remain resolved against the live store.
#[derive(Debug)]
pub struct ResolvedDnsResolverDefault {
    candidate: Option<ResolverCandidate>,
    cache_ttl: Duration,
}

impl ResolvedDnsResolverDefault {
    fn resolve(owner: &EdgionGatewayConfig) -> Self {
        let config = owner
            .spec
            .dns_resolver
            .as_ref()
            .expect("selected DNS owner has a DNS field");
        Self {
            candidate: candidate_from_egc_default(owner),
            cache_ttl: parse_duration(&config.cache_ttl).unwrap_or(crate::backends::DEFAULT_DNS_CACHE_TTL),
        }
    }

    pub fn candidate(&self) -> Option<&ResolverCandidate> {
        self.candidate.as_ref()
    }

    pub fn cache_ttl(&self) -> Duration {
        self.cache_ttl
    }
}

struct EdgionGatewayConfigSnapshot {
    configs: HashMap<String, Arc<EdgionGatewayConfig>>,
    runtime_configs: HashMap<String, Arc<ResolvedGatewayConfig>>,
    dns_resolver_default: Option<Arc<ResolvedDnsResolverDefault>>,
}

impl EdgionGatewayConfigSnapshot {
    fn new(configs: HashMap<String, Arc<EdgionGatewayConfig>>) -> Self {
        let dns_resolver_default = configs
            .iter()
            .filter(|(_, config)| config.spec.dns_resolver.is_some())
            .min_by(|a, b| a.0.cmp(b.0))
            .map(|(_, owner)| ResolvedDnsResolverDefault::resolve(owner))
            .map(Arc::new);
        let runtime_configs = configs
            .values()
            .map(|config| (config.name_any(), Arc::new(ResolvedGatewayConfig::resolve(config))))
            .collect();
        Self {
            configs,
            runtime_configs,
            dns_resolver_default,
        }
    }

    fn empty() -> Self {
        Self::new(HashMap::new())
    }
}

/// Global EdgionGatewayConfig store and its request-path-ready derived policy.
///
/// Raw resources and derived policies share one immutable source snapshot.
/// Listener bindings publish independently with per-EGC atomicity. Human-readable body
/// sizes, the upstream RemoteIp header, and the preflight policy are resolved
/// while constructing the snapshot, never per request.
static EDGION_GATEWAY_CONFIG_STORE: std::sync::LazyLock<ArcSwap<EdgionGatewayConfigSnapshot>> =
    std::sync::LazyLock::new(|| ArcSwap::from_pointee(EdgionGatewayConfigSnapshot::empty()));

/// Serialize snapshot read-modify-write cycles and listener binding.
///
/// The normal ClientCache callback path is already per-kind serial, but
/// ConfHandler is Send + Sync and may gain another rebuild trigger. Keeping the
/// guarantee here prevents a later concurrent writer from publishing a snapshot
/// derived from stale state. Request readers never acquire this lock.
static EDGION_GATEWAY_CONFIG_WRITE_LOCK: std::sync::LazyLock<Mutex<HashMap<String, Weak<GatewayConfigHandle>>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Stable listener binding. Only publication replaces its immutable generation.
#[derive(Debug)]
pub struct GatewayConfigHandle {
    current: ArcSwap<ResolvedGatewayConfig>,
}

impl GatewayConfigHandle {
    /// Capture one generation without a resource-table lookup or blocking lock.
    #[inline]
    pub fn load(&self) -> Arc<ResolvedGatewayConfig> {
        self.current.load_full()
    }
}

pub fn default_gateway_config() -> Arc<ResolvedGatewayConfig> {
    static DEFAULT: std::sync::LazyLock<Arc<ResolvedGatewayConfig>> =
        std::sync::LazyLock::new(|| Arc::new(ResolvedGatewayConfig::default()));
    Arc::clone(&DEFAULT)
}

/// Bind on listener construction, never on the request or connection path.
/// Binding and publication share the writer lock so neither can miss an update.
pub fn bind_gateway_config(name: Option<&str>) -> Arc<GatewayConfigHandle> {
    static DEFAULT: std::sync::LazyLock<Arc<GatewayConfigHandle>> = std::sync::LazyLock::new(|| {
        Arc::new(GatewayConfigHandle {
            current: ArcSwap::from(default_gateway_config()),
        })
    });
    let Some(name) = name else {
        return Arc::clone(&DEFAULT);
    };
    let mut bindings = EDGION_GATEWAY_CONFIG_WRITE_LOCK.lock();
    bindings.retain(|_, handle| handle.strong_count() != 0);
    if let Some(handle) = bindings.get(name).and_then(Weak::upgrade) {
        return handle;
    }
    let config = EDGION_GATEWAY_CONFIG_STORE
        .load()
        .runtime_configs
        .get(name)
        .cloned()
        .unwrap_or_else(default_gateway_config);
    let handle = Arc::new(GatewayConfigHandle {
        current: ArcSwap::from(config),
    });
    bindings.insert(name.to_owned(), Arc::downgrade(&handle));
    handle
}

/// Publish under the writer lock. Each live name retains its handle across
/// removal/recreation; expired bindings retain no runtime and are pruned here.
/// Atomicity is per EGC, not across resources or the raw-resource/admin view.
fn publish_snapshot(
    configs: HashMap<String, Arc<EdgionGatewayConfig>>,
    bindings: &mut HashMap<String, Weak<GatewayConfigHandle>>,
) {
    let snapshot = Arc::new(EdgionGatewayConfigSnapshot::new(configs));
    bindings.retain(|name, weak| {
        let Some(handle) = weak.upgrade() else {
            return false;
        };
        handle.current.store(
            snapshot
                .runtime_configs
                .get(name)
                .cloned()
                .unwrap_or_else(default_gateway_config),
        );
        true
    });
    EDGION_GATEWAY_CONFIG_STORE.store(snapshot);
}

#[derive(Default, serde::Serialize)]
pub struct GatewayConfigBindingStats {
    pub count: usize,
    pub expired_count: usize,
    pub keys: Vec<String>,
}

/// Debug-only inventory: observing stats must not hide expired bindings.
pub fn gateway_config_binding_stats() -> GatewayConfigBindingStats {
    let bindings = EDGION_GATEWAY_CONFIG_WRITE_LOCK.lock();
    let mut keys: Vec<_> = bindings.keys().cloned().collect();
    keys.sort();
    GatewayConfigBindingStats {
        count: bindings.len(),
        expired_count: bindings.values().filter(|handle| handle.strong_count() == 0).count(),
        keys,
    }
}

/// Count EdgionGatewayConfig resources in the store
pub fn count_edgion_gateway_configs() -> usize {
    EDGION_GATEWAY_CONFIG_STORE.load().configs.len()
}

#[cfg(test)]
include!("../../../tests/support/config/edgion_gateway/conf_handler_impl_test/378_function_item.rs");

#[cfg(test)]
include!("../../../tests/support/config/edgion_gateway/conf_handler_impl_test/386_function_item.rs");

#[cfg(test)]
include!("../../../tests/support/config/edgion_gateway/conf_handler_impl_test/395_function_item.rs");

#[cfg(test)]
include!("../../../tests/support/config/edgion_gateway/conf_handler_impl_test/400_function_item.rs");

/// The data-plane default DNS resolver (`spec.dnsResolver`) from the effectively
/// singular EGC store, if any EGC declares one.
///
/// Cheap hot-path accessor: it clones only the pre-resolved `Arc`, not the whole
/// store, and picks deterministically by name (lowest key wins) so the choice is
/// stable across restarts when more than one EGC sets a default.
pub fn get_egc_dns_resolver_default() -> Option<Arc<ResolvedDnsResolverDefault>> {
    EDGION_GATEWAY_CONFIG_STORE.load().dns_resolver_default.clone()
}

/// ConfHandler implementation for EdgionGatewayConfig
pub struct EdgionGatewayConfigHandler;

impl EdgionGatewayConfigHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EdgionGatewayConfigHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Default webhook response-body cap when no `EdgionGatewayConfig` sets `link_sys`.
/// Must mirror `WEBHOOK_GLOBAL_MAX`'s initial value (`manager.rs`) and the schema
/// default (`default_webhook_max_response_bytes`).
const DEFAULT_WEBHOOK_MAX_RESPONSE_BYTES: usize = 16384;

/// Default LinkSys instance cap when no `EdgionGatewayConfig` sets `link_sys`.
/// Must mirror `LINK_SYS_MAX_INSTANCES_PER_KIND`'s initial value (`store.rs`)
/// and the schema default (`LINK_SYS_DEFAULT_MAX_INSTANCES_PER_KIND`).
const DEFAULT_LINK_SYS_MAX_INSTANCES_PER_KIND: usize = 200;

/// Inclusive upper bound mirrored from `LINK_SYS_MAX_INSTANCES_PER_KIND_LIMIT`.
const LINK_SYS_MAX_INSTANCES_PER_KIND_LIMIT: usize = 10_000;

/// Default LB degrade threshold when no `EdgionGatewayConfig` sets `load_balancing`.
/// Must mirror `GLOBAL_DEFAULT`'s initial value (`degrade_threshold.rs`): 0 =
/// degrade mode disabled (opt-in).
const DEFAULT_LB_DEGRADE_THRESHOLD: u8 = 0;

/// Deterministically recompute the process-global LinkSys / load-balancing values
/// from the **entire** config store and apply them.
///
/// `EdgionGatewayConfig` is cluster-scoped, so several instances may coexist and
/// the store iterates in non-deterministic hash order. Applying each config's
/// global value in turn would make the surviving value depend on iteration order,
/// and the delete path would never restore the default. Instead we aggregate
/// across all configs with an order-independent rule and fall back to the schema
/// default when no config sets the field:
///   - webhook response cap: `min` — the strictest cap wins.
///   - LinkSys instances per kind: `min` — the strictest cap wins.
///   - LB degrade threshold:   `max` — degrade mode engages as early as any config asks.
fn recompute_global_config(store: &HashMap<String, Arc<EdgionGatewayConfig>>) -> bool {
    #[cfg(test)]
    let _cap = crate::link_sys::runtime::store::LINK_SYS_CAP_TEST_LOCK.lock();
    let webhook_max = store
        .values()
        .filter_map(|config| config.spec.link_sys.as_ref())
        .map(|link_sys| link_sys.webhook_max_response_bytes)
        .min()
        .unwrap_or(DEFAULT_WEBHOOK_MAX_RESPONSE_BYTES);
    configure_webhook_global_max(webhook_max);

    let max_instances = store
        .values()
        .filter_map(|config| config.spec.link_sys.as_ref())
        .map(|link_sys| link_sys.max_instances_per_kind)
        .filter(|&n| (1..=LINK_SYS_MAX_INSTANCES_PER_KIND_LIMIT).contains(&n))
        .min()
        .unwrap_or(DEFAULT_LINK_SYS_MAX_INSTANCES_PER_KIND);
    configure_link_sys_max_instances_per_kind(max_instances);

    // Drop out-of-range thresholds before aggregating so one invalid config
    // (`> 100`) cannot poison the `max` and silently disable degrade mode for every
    // service — mirrors how the per-service annotation parser ignores invalid
    // values and falls back. If all configs are invalid the aggregate is empty and
    // we reset to the default.
    let degrade_threshold = store
        .values()
        .filter_map(|config| config.spec.load_balancing.as_ref())
        .map(|lb| lb.degrade_threshold)
        .filter(|&threshold| threshold <= 100)
        .max()
        .unwrap_or(DEFAULT_LB_DEGRADE_THRESHOLD);
    crate::backends::degrade_threshold::configure_lb_degrade_default(degrade_threshold);

    // Aggregate the cluster-wide outbound TLS default. Safest-wins on verify;
    // CA material and the client mTLS identity are independent policy axes and
    // are each adopted only when exactly one config declares that axis.
    // Configs that do not set outbound_tls at all default to verify=true (the safe
    // default), so the verify aggregate is true whenever any config omits the block.
    use edgion_common::link_sys_core::outbound_tls::{set_outbound_global_default, GlobalOutboundTlsSnapshot};
    let snapshot = Arc::new(GlobalOutboundTlsSnapshot::from_configs(store.values().cloned()));
    if snapshot.ca_conflict() {
        crate::observe::metrics::record_config_conflict("outbound_ca");
        tracing::warn!(
            component = "outbound_tls",
            "multiple EdgionGatewayConfig declare conflicting outbound CA defaults; falling back to System"
        );
    }
    if snapshot.client_conflict() {
        crate::observe::metrics::record_config_conflict("outbound_client_identity");
        tracing::warn!(
            component = "outbound_tls",
            "multiple EdgionGatewayConfig declare conflicting outbound client identities; inherited outbound TLS is disabled"
        );
    }
    set_outbound_global_default(snapshot)
}

impl ConfHandler<EdgionGatewayConfig> for EdgionGatewayConfigHandler {
    fn full_set(&self, data: &HashMap<String, Arc<EdgionGatewayConfig>>) {
        let mut bindings = EDGION_GATEWAY_CONFIG_WRITE_LOCK.lock();
        tracing::info!(
            count = data.len(),
            "EdgionGatewayConfig full_set: received {} EdgionGatewayConfig resources",
            data.len()
        );
        let configs = data.clone();
        let outbound_tls_changed = recompute_global_config(&configs);
        publish_snapshot(configs, &mut bindings);
        drop(bindings);
        if outbound_tls_changed {
            crate::link_sys::get_global_link_sys_store().rebuild_clients();
        }
        crate::plugins::publication::global_publisher().replace_gateway_configs(data.clone());
        crate::ai::reconcile_all_adaptive_plugin_dependents();
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<EdgionGatewayConfig>>,
        update: HashMap<String, Arc<EdgionGatewayConfig>>,
        remove: HashSet<String>,
    ) {
        let policy_inputs = add
            .iter()
            .chain(update.iter())
            .map(|(key, value)| (key.clone(), Arc::clone(value)))
            .collect();
        let mut bindings = EDGION_GATEWAY_CONFIG_WRITE_LOCK.lock();
        let mut configs = EDGION_GATEWAY_CONFIG_STORE.load_full().configs.clone();

        if !add.is_empty() {
            tracing::info!(
                count = add.len(),
                "EdgionGatewayConfig partial_update: added {} EdgionGatewayConfig resources",
                add.len()
            );
            for (key, config) in add {
                tracing::debug!(key = %key, name = %config.name_any(), "EdgionGatewayConfig added");
                configs.insert(key, config);
            }
        }

        if !update.is_empty() {
            tracing::info!(
                count = update.len(),
                "EdgionGatewayConfig partial_update: updated {} EdgionGatewayConfig resources",
                update.len()
            );
            for (key, config) in update {
                tracing::debug!(key = %key, name = %config.name_any(), "EdgionGatewayConfig updated");
                if configs.insert(key.clone(), config).is_none() {
                    tracing::warn!(key = %key, "EdgionGatewayConfig update: key not found in store, inserting");
                }
            }
        }

        if !remove.is_empty() {
            tracing::info!(
                count = remove.len(),
                "EdgionGatewayConfig partial_update: removed {} EdgionGatewayConfig resources",
                remove.len()
            );
            for key in &remove {
                tracing::debug!(key = %key, "EdgionGatewayConfig removed");
                if configs.remove(key).is_none() {
                    tracing::debug!(key = %key, "EdgionGatewayConfig remove: key not found in store");
                }
            }
        }

        // Recompute the process-global values from the post-mutation store so the
        // result is deterministic across instances and the delete path restores
        // the default once no config carries the field.
        let outbound_tls_changed = recompute_global_config(&configs);
        publish_snapshot(configs, &mut bindings);
        drop(bindings);
        if outbound_tls_changed {
            crate::link_sys::get_global_link_sys_store().rebuild_clients();
        }
        crate::plugins::publication::global_publisher().update_gateway_configs(policy_inputs, &remove);
        crate::ai::reconcile_all_adaptive_plugin_dependents();
    }
}

/// Create EdgionGatewayConfig handler
pub fn create_edgion_gateway_config_handler() -> Box<dyn ConfHandler<EdgionGatewayConfig> + Send + Sync> {
    Box::new(EdgionGatewayConfigHandler::new())
}

#[cfg(test)]
#[path = "../../../tests/unit/config/edgion_gateway/conf_handler_impl_test.rs"]
mod tests;
