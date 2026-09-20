use crate::link_sys::providers::webhook::manager::configure_webhook_global_max;
use crate::link_sys::runtime::store::configure_link_sys_max_instances_per_kind;
use crate::request_body::ResolvedRequestBodyConfig;
use crate::routes::http::proxy_http::preflight_handler::ResolvedPreflightPolicy;
use arc_swap::ArcSwap;
use edgion_common::conf_sync::traits::{arc_map_to_owned, ConfHandler};
use edgion_common::dns::resolver_select::{candidate_from_egc_default, ResolverCandidate};
use edgion_resources::duration::parse_duration;
use edgion_resources::prelude_resources::EdgionGatewayConfig;
use edgion_resources::resources::edgion_gateway_config::{
    resolve_tcp_connect_timeout, resolve_tcp_idle_timeout, DnsResolverConfig,
};
use kube::ResourceExt;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
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

/// Request-scoped HTTP configuration resolved from one EdgionGatewayConfig generation.
///
/// A new request clones this value from the current immutable configuration
/// snapshot. All fields therefore come from the same publication epoch, while
/// in-flight requests retain their captured values across hot reloads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedHttpRequestConfig {
    pub(crate) request_body: ResolvedRequestBodyConfig,
    pub(crate) remote_ip_header: http::HeaderName,
    pub(crate) preflight_policy: ResolvedPreflightPolicy,
    pub(crate) reject_duplicate_host: bool,
}

impl ResolvedHttpRequestConfig {
    fn resolve(config: &EdgionGatewayConfig) -> Self {
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

        Self {
            request_body: ResolvedRequestBodyConfig::resolve(&config.spec.request_body),
            remote_ip_header,
            preflight_policy: ResolvedPreflightPolicy::resolve(config.spec.preflight_policy.as_ref()),
            reject_duplicate_host: config
                .spec
                .security_protect
                .as_ref()
                .map(|security| security.reject_duplicate_host)
                .unwrap_or(true),
        }
    }
}

impl Default for ResolvedHttpRequestConfig {
    fn default() -> Self {
        Self {
            request_body: ResolvedRequestBodyConfig::default(),
            remote_ip_header: default_remote_ip_header(),
            preflight_policy: ResolvedPreflightPolicy::default(),
            reject_duplicate_host: true,
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
    fn resolve(config: &DnsResolverConfig) -> Self {
        Self {
            candidate: candidate_from_egc_default(config),
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
    configs: HashMap<String, EdgionGatewayConfig>,
    http_request_configs: HashMap<String, ResolvedHttpRequestConfig>,
    l4_timeouts: HashMap<String, ResolvedL4Timeouts>,
    dns_resolver_default: Option<Arc<ResolvedDnsResolverDefault>>,
}

impl EdgionGatewayConfigSnapshot {
    fn new(configs: HashMap<String, EdgionGatewayConfig>) -> Self {
        let dns_resolver_default = configs
            .iter()
            .filter(|(_, config)| config.spec.dns_resolver.is_some())
            .min_by(|a, b| a.0.cmp(b.0))
            .and_then(|(_, config)| config.spec.dns_resolver.as_ref())
            .map(ResolvedDnsResolverDefault::resolve)
            .map(Arc::new);
        let http_request_configs = configs
            .values()
            .map(|config| (config.name_any(), ResolvedHttpRequestConfig::resolve(config)))
            .collect();
        let l4_timeouts = configs
            .values()
            .map(|config| (config.name_any(), ResolvedL4Timeouts::resolve(config)))
            .collect();
        Self {
            configs,
            http_request_configs,
            l4_timeouts,
            dns_resolver_default,
        }
    }

    fn empty() -> Self {
        Self::new(HashMap::new())
    }
}

/// Global EdgionGatewayConfig store and its request-path-ready derived policy.
///
/// All views are published in one immutable snapshot so an EGC update cannot
/// expose values from different generations to one lookup. Human-readable body
/// sizes, the upstream RemoteIp header, and the preflight policy are resolved
/// while constructing the snapshot, never per request.
static EDGION_GATEWAY_CONFIG_STORE: std::sync::LazyLock<ArcSwap<EdgionGatewayConfigSnapshot>> =
    std::sync::LazyLock::new(|| ArcSwap::from_pointee(EdgionGatewayConfigSnapshot::empty()));

/// Serialize snapshot read-modify-write cycles.
///
/// The normal ClientCache callback path is already per-kind serial, but
/// ConfHandler is Send + Sync and may gain another rebuild trigger. Keeping the
/// guarantee here prevents a later concurrent writer from publishing a snapshot
/// derived from stale state. Request readers never acquire this lock.
static EDGION_GATEWAY_CONFIG_WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Count EdgionGatewayConfig resources in the store
pub fn count_edgion_gateway_configs() -> usize {
    EDGION_GATEWAY_CONFIG_STORE.load().configs.len()
}

/// Query EdgionGatewayConfig by name (cluster-scoped: key == name).
/// Test-only: production reads go through `ConfigClient::get_edgion_gateway_config`
/// against the conf_sync ClientCache, not this store.
#[cfg(test)]
pub fn get_edgion_gateway_config_by_name(name: &str) -> Option<EdgionGatewayConfig> {
    EDGION_GATEWAY_CONFIG_STORE.load().configs.get(name).cloned()
}

/// List all EdgionGatewayConfig resources.
/// Test-only: production reads go through `ConfigClient::list_edgion_gateway_configs`
/// against the conf_sync ClientCache, not this store.
#[cfg(test)]
pub fn list_edgion_gateway_configs() -> Vec<EdgionGatewayConfig> {
    let mut items: Vec<EdgionGatewayConfig> = EDGION_GATEWAY_CONFIG_STORE.load().configs.values().cloned().collect();
    items.sort_by_key(|a| a.name_any());
    items
}

/// Load the HTTP request configuration referenced by a listener's GatewayClass.
///
/// `None`, a missing EGC, and a deleted EGC all resolve to the built-in safe
/// default. The hot path performs one ArcSwap load, one bounded map lookup, and
/// a shallow clone of the compact resolved value.
#[inline]
pub(crate) fn get_resolved_http_request_config(config_name: Option<&str>) -> ResolvedHttpRequestConfig {
    let snapshot = EDGION_GATEWAY_CONFIG_STORE.load();
    config_name
        .and_then(|name| snapshot.http_request_configs.get(name))
        .cloned()
        .unwrap_or_default()
}

/// Load the L4 timeout policy referenced by a listener's GatewayClass.
///
/// The configuration update path has already parsed the duration strings. A
/// connection performs one lock-free snapshot load and one bounded map lookup,
/// then keeps the copied value for its lifetime. Missing and deleted configs,
/// and GatewayClasses without a parametersRef, use the built-in safe defaults.
#[inline]
pub fn get_resolved_l4_timeouts(config_name: Option<&str>) -> ResolvedL4Timeouts {
    let snapshot = EDGION_GATEWAY_CONFIG_STORE.load();
    config_name
        .and_then(|name| snapshot.l4_timeouts.get(name))
        .copied()
        .unwrap_or_default()
}

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
fn recompute_global_config(store: &HashMap<String, EdgionGatewayConfig>) -> bool {
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
    let outbound_blocks: Vec<_> = store
        .values()
        .map(|c| c.spec.outbound_tls.clone().unwrap_or_default())
        .collect();
    let mut agg = edgion_common::link_sys_core::outbound_tls::aggregate_verify(&outbound_blocks);
    let ca_blocks: Vec<_> = outbound_blocks
        .iter()
        .filter(|b| {
            b.validation.as_ref().is_some_and(|v| {
                v.ca_certificate_refs.as_ref().is_some_and(|r| !r.is_empty()) || v.well_known_ca_certificates.is_some()
            })
        })
        .collect();
    match ca_blocks.as_slice() {
        [one] => {
            // A single non-empty validation (refs or wellKnown) is adopted as
            // the cluster-wide default. The Controller has already resolved the CA
            // Secrets into `resolved_ca_certificates` and shipped them via conf_sync,
            // so the per-call resolver can read them for inheriting calls.
            agg.validation = one.validation.clone();
            agg.resolved_ca_certificates = one.resolved_ca_certificates.clone();
        }
        [] => {}
        _ => {
            crate::observe::metrics::record_config_conflict("outbound_ca");
            tracing::warn!(
                component = "outbound_tls",
                "multiple EdgionGatewayConfig declare conflicting outbound CA defaults; falling back to System"
            );
        }
    }
    let mut client_identity_blocks: Vec<_> = outbound_blocks
        .iter()
        .filter(|block| block.client_certificate_ref.is_some())
        .collect();
    client_identity_blocks.sort_by_key(|block| {
        block.client_certificate_ref.as_ref().map(|reference| {
            (
                reference.namespace.clone().unwrap_or_default(),
                reference.name.clone(),
                reference.group.clone().unwrap_or_default(),
                reference.kind.clone().unwrap_or_else(|| "Secret".to_string()),
            )
        })
    });
    match client_identity_blocks.as_slice() {
        [one] => {
            agg.client_certificate_ref = one.client_certificate_ref.clone();
            agg.resolved_client_certificate = one.resolved_client_certificate.clone();
        }
        [] => {}
        [first, ..] => {
            // Keep a declared ref but no resolved material so every inheriting
            // resolver fails closed instead of silently sending no client cert.
            agg.client_certificate_ref = first.client_certificate_ref.clone();
            agg.resolved_client_certificate = None;
            crate::observe::metrics::record_config_conflict("outbound_client_identity");
            tracing::warn!(
                component = "outbound_tls",
                "multiple EdgionGatewayConfig declare conflicting outbound client identities; inherited outbound TLS is disabled"
            );
        }
    }
    edgion_common::link_sys_core::outbound_tls::set_outbound_global_default(agg)
}

impl ConfHandler<EdgionGatewayConfig> for EdgionGatewayConfigHandler {
    fn full_set(&self, data: &HashMap<String, Arc<EdgionGatewayConfig>>) {
        let _write_guard = EDGION_GATEWAY_CONFIG_WRITE_LOCK.lock();
        tracing::info!(
            count = data.len(),
            "EdgionGatewayConfig full_set: received {} EdgionGatewayConfig resources",
            data.len()
        );
        let configs = arc_map_to_owned(data);
        let outbound_tls_changed = recompute_global_config(&configs);
        EDGION_GATEWAY_CONFIG_STORE.store(Arc::new(EdgionGatewayConfigSnapshot::new(configs)));
        drop(_write_guard);
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
        let _write_guard = EDGION_GATEWAY_CONFIG_WRITE_LOCK.lock();
        let mut configs = EDGION_GATEWAY_CONFIG_STORE.load_full().configs.clone();

        if !add.is_empty() {
            tracing::info!(
                count = add.len(),
                "EdgionGatewayConfig partial_update: added {} EdgionGatewayConfig resources",
                add.len()
            );
            for (key, config) in add {
                tracing::debug!(key = %key, name = %config.name_any(), "EdgionGatewayConfig added");
                configs.insert(key, (*config).clone());
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
                if configs.insert(key.clone(), (*config).clone()).is_none() {
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
        EDGION_GATEWAY_CONFIG_STORE.store(Arc::new(EdgionGatewayConfigSnapshot::new(configs)));
        drop(_write_guard);
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
mod tests {
    use super::*;
    use crate::routes::http::proxy_http::preflight_handler::ResolvedPreflightMode;
    use edgion_resources::resources::edgion_gateway_config::{
        EdgionGatewayConfigSpec, PreflightMode, PreflightPolicy, TcpTimeout, DEFAULT_TCP_CONNECT, DEFAULT_TCP_IDLE,
    };
    use std::collections::HashMap;

    // Serialize against every test module that mutates gateway-wide stores.
    use crate::backends::degrade_threshold::GLOBAL_STORE_TEST_LOCK as TEST_MUTEX;

    fn make_config(name: &str) -> EdgionGatewayConfig {
        EdgionGatewayConfig::new(
            name,
            EdgionGatewayConfigSpec {
                server: None,
                http_timeout: None,
                max_retries: 3,
                real_ip: None,
                forwarded_headers: Default::default(),
                security_protect: None,
                request_body: Default::default(),
                global_plugins_ref: None,
                plugin_policy: None,
                access_log_extern: None,
                preflight_policy: None,
                link_sys: None,
                tcp_timeout: None,
                load_balancing: None,
                outbound_tls: None,
                dns_resolver: None,
            },
        )
    }

    fn make_config_with_l4_timeouts(name: &str, connect: &str, idle: &str) -> EdgionGatewayConfig {
        let mut config = make_config(name);
        config.spec.tcp_timeout = Some(TcpTimeout {
            connect_timeout: connect.to_string(),
            idle_timeout: idle.to_string(),
        });
        config
    }

    fn clear_store() {
        let _write_guard = EDGION_GATEWAY_CONFIG_WRITE_LOCK.lock();
        EDGION_GATEWAY_CONFIG_STORE.store(Arc::new(EdgionGatewayConfigSnapshot::empty()));
    }

    #[test]
    fn test_full_set_replaces_existing() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = EdgionGatewayConfigHandler::new();

        let mut data = HashMap::new();
        data.insert("foo".to_string(), Arc::new(make_config("foo")));
        data.insert("bar".to_string(), Arc::new(make_config("bar")));
        handler.full_set(&data);
        assert_eq!(count_edgion_gateway_configs(), 2);

        // full_set again with only one item — old entries must be gone
        let mut data2 = HashMap::new();
        data2.insert("baz".to_string(), Arc::new(make_config("baz")));
        handler.full_set(&data2);
        assert_eq!(count_edgion_gateway_configs(), 1);
    }

    #[test]
    fn test_add_deduplicates() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = EdgionGatewayConfigHandler::new();

        let mut add = HashMap::new();
        add.insert("foo".to_string(), Arc::new(make_config("foo")));
        handler.partial_update(add.clone(), HashMap::new(), std::collections::HashSet::new());
        // add same key again
        handler.partial_update(add, HashMap::new(), std::collections::HashSet::new());

        assert_eq!(
            count_edgion_gateway_configs(),
            1,
            "duplicate add must not create two entries"
        );
    }

    #[test]
    fn test_update_overwrites_existing() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = EdgionGatewayConfigHandler::new();

        let mut add = HashMap::new();
        add.insert("foo".to_string(), Arc::new(make_config("foo")));
        handler.partial_update(add, HashMap::new(), std::collections::HashSet::new());

        let mut updated = make_config("foo");
        updated.spec.max_retries = 99;
        let mut upd = HashMap::new();
        upd.insert("foo".to_string(), Arc::new(updated));
        handler.partial_update(HashMap::new(), upd, std::collections::HashSet::new());

        let result = get_edgion_gateway_config_by_name("foo").expect("must exist after update");
        assert_eq!(result.spec.max_retries, 99);
        assert_eq!(count_edgion_gateway_configs(), 1);
    }

    #[test]
    fn invalid_remote_ip_header_preserves_valid_body_policy() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = EdgionGatewayConfigHandler::new();

        let mut invalid = make_config("foo");
        invalid.spec.forwarded_headers.remote_ip_header = "Host".to_string();
        invalid.spec.request_body.enabled = false;
        invalid.spec.request_body.max_memory_buffer_size = Some("384KiB".to_string());
        invalid.spec.request_body.max_body_size = Some("3MiB".to_string());
        invalid.spec.request_body.storage_operation_timeout = "3s".to_string();
        handler.partial_update(
            HashMap::from([("foo".to_string(), Arc::new(invalid))]),
            HashMap::new(),
            HashSet::new(),
        );

        let resolved = get_resolved_http_request_config(Some("foo"));
        assert_eq!(resolved.remote_ip_header, "x-real-ip");
        assert_eq!(
            resolved.request_body,
            ResolvedRequestBodyConfig {
                enabled: false,
                memory_buffer_bytes: 384 * 1024,
                max_body_size_bytes: 3 * 1024 * 1024,
                storage_operation_timeout: Duration::from_secs(3),
            }
        );
    }

    #[test]
    fn test_update_miss_inserts() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = EdgionGatewayConfigHandler::new();

        // update on empty store must insert (not silently discard)
        let mut upd = HashMap::new();
        upd.insert("ghost".to_string(), Arc::new(make_config("ghost")));
        handler.partial_update(HashMap::new(), upd, std::collections::HashSet::new());

        assert_eq!(count_edgion_gateway_configs(), 1, "update miss must insert");
    }

    #[test]
    fn test_remove_works() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = EdgionGatewayConfigHandler::new();

        let mut add = HashMap::new();
        add.insert("foo".to_string(), Arc::new(make_config("foo")));
        add.insert("bar".to_string(), Arc::new(make_config("bar")));
        handler.partial_update(add, HashMap::new(), std::collections::HashSet::new());

        let mut remove = std::collections::HashSet::new();
        remove.insert("foo".to_string());
        handler.partial_update(HashMap::new(), HashMap::new(), remove);

        assert_eq!(count_edgion_gateway_configs(), 1);
        assert!(get_edgion_gateway_config_by_name("foo").is_none());
        assert!(get_edgion_gateway_config_by_name("bar").is_some());
    }

    #[test]
    fn test_remove_miss_does_not_panic() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = EdgionGatewayConfigHandler::new();

        let mut remove = std::collections::HashSet::new();
        remove.insert("nonexistent".to_string());
        // must not panic
        handler.partial_update(HashMap::new(), HashMap::new(), remove);
        assert_eq!(count_edgion_gateway_configs(), 0);
    }

    #[test]
    fn http_request_config_updates_joint_pair_and_pins_in_flight_value() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = EdgionGatewayConfigHandler::new();

        let mut initial = make_config("http-request-policy");
        initial.spec.forwarded_headers.remote_ip_header = "X-Initial-Remote-IP".to_string();
        initial.spec.request_body.enabled = false;
        initial.spec.request_body.max_memory_buffer_size = Some("256KiB".to_string());
        initial.spec.request_body.max_body_size = Some("1MiB".to_string());
        initial.spec.request_body.storage_operation_timeout = "750ms".to_string();
        initial.spec.preflight_policy = Some(PreflightPolicy {
            mode: PreflightMode::AllOptions,
            status_code: 299,
        });
        initial.spec.security_protect =
            Some(serde_json::from_value(serde_json::json!({"rejectDuplicateHost": false})).unwrap());
        let mut add = HashMap::new();
        add.insert("http-request-policy".to_string(), Arc::new(initial));
        handler.partial_update(add, HashMap::new(), HashSet::new());

        let in_flight = get_resolved_http_request_config(Some("http-request-policy"));
        assert_eq!(in_flight.remote_ip_header, "x-initial-remote-ip");
        assert!(!in_flight.request_body.enabled);
        assert_eq!(in_flight.request_body.memory_buffer_bytes, 256 * 1024);
        assert_eq!(in_flight.request_body.max_body_size_bytes, 1024 * 1024);
        assert_eq!(
            in_flight.request_body.storage_operation_timeout,
            std::time::Duration::from_millis(750)
        );
        assert_eq!(
            in_flight.preflight_policy,
            ResolvedPreflightPolicy {
                mode: ResolvedPreflightMode::AllOptions,
                status_code: 299,
            }
        );
        assert!(!in_flight.reject_duplicate_host);

        let mut updated = make_config("http-request-policy");
        updated.spec.forwarded_headers.remote_ip_header = "X-Updated-Remote-IP".to_string();
        updated.spec.request_body.max_memory_buffer_size = Some("512KiB".to_string());
        updated.spec.request_body.max_body_size = Some("2MiB".to_string());
        updated.spec.request_body.storage_operation_timeout = "2s".to_string();
        updated.spec.preflight_policy = Some(PreflightPolicy {
            mode: PreflightMode::CorsStandard,
            status_code: 205,
        });
        updated.spec.security_protect =
            Some(serde_json::from_value(serde_json::json!({"rejectDuplicateHost": true})).unwrap());
        let mut update = HashMap::new();
        update.insert("http-request-policy".to_string(), Arc::new(updated));
        handler.partial_update(HashMap::new(), update, HashSet::new());

        let next = get_resolved_http_request_config(Some("http-request-policy"));
        assert_eq!(next.remote_ip_header, "x-updated-remote-ip");
        assert!(next.request_body.enabled);
        assert_eq!(next.request_body.memory_buffer_bytes, 512 * 1024);
        assert_eq!(next.request_body.max_body_size_bytes, 2 * 1024 * 1024);
        assert_eq!(next.request_body.storage_operation_timeout, Duration::from_secs(2));
        assert_eq!(
            next.preflight_policy,
            ResolvedPreflightPolicy {
                mode: ResolvedPreflightMode::CorsStandard,
                status_code: 205,
            }
        );
        assert!(next.reject_duplicate_host);
        assert_eq!(in_flight.remote_ip_header, "x-initial-remote-ip");
        assert!(!in_flight.request_body.enabled);
        assert_eq!(in_flight.request_body.memory_buffer_bytes, 256 * 1024);
        assert_eq!(in_flight.request_body.max_body_size_bytes, 1024 * 1024);
        assert_eq!(
            in_flight.request_body.storage_operation_timeout,
            Duration::from_millis(750)
        );
        assert_eq!(
            in_flight.preflight_policy,
            ResolvedPreflightPolicy {
                mode: ResolvedPreflightMode::AllOptions,
                status_code: 299,
            }
        );
        assert!(!in_flight.reject_duplicate_host);

        handler.partial_update(
            HashMap::new(),
            HashMap::new(),
            HashSet::from(["http-request-policy".to_string()]),
        );
        let default = ResolvedHttpRequestConfig::default();
        assert_eq!(get_resolved_http_request_config(Some("http-request-policy")), default);
        assert_eq!(get_resolved_http_request_config(Some("missing")), default);
        assert_eq!(get_resolved_http_request_config(None), default);
        assert!(default.reject_duplicate_host);
    }

    #[test]
    fn invalid_preflight_status_is_preserved_for_runtime_fail_close() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = EdgionGatewayConfigHandler::new();
        let mut config = make_config("invalid-preflight-policy");
        config.spec.preflight_policy = Some(PreflightPolicy {
            mode: PreflightMode::AllOptions,
            status_code: 700,
        });

        handler.full_set(&HashMap::from([(
            "invalid-preflight-policy".to_string(),
            Arc::new(config),
        )]));

        assert_eq!(
            get_resolved_http_request_config(Some("invalid-preflight-policy")).preflight_policy,
            ResolvedPreflightPolicy {
                mode: ResolvedPreflightMode::AllOptions,
                status_code: 700,
            }
        );
    }

    #[test]
    fn invalid_body_fields_preserve_valid_remote_ip_header() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = EdgionGatewayConfigHandler::new();

        let mut config = make_config("invalid-body-policy");
        config.spec.forwarded_headers.remote_ip_header = "X-Valid-Remote-IP".to_string();
        config.spec.request_body.enabled = false;
        config.spec.request_body.default_memory_buffer_size = "384KiB".to_string();
        config.spec.request_body.max_memory_buffer_size = Some("invalid".to_string());
        config.spec.request_body.default_max_body_size = "3MiB".to_string();
        config.spec.request_body.max_body_size = Some("0".to_string());
        config.spec.request_body.storage_operation_timeout = "invalid".to_string();
        let mut data = HashMap::new();
        data.insert("invalid-body-policy".to_string(), Arc::new(config));
        handler.full_set(&data);

        let resolved = get_resolved_http_request_config(Some("invalid-body-policy"));
        assert_eq!(resolved.remote_ip_header, "x-valid-remote-ip");
        assert_eq!(
            resolved.request_body,
            ResolvedRequestBodyConfig {
                enabled: false,
                memory_buffer_bytes: 384 * 1024,
                max_body_size_bytes: 3 * 1024 * 1024,
                storage_operation_timeout: ResolvedRequestBodyConfig::default().storage_operation_timeout,
            }
        );
    }

    #[test]
    fn l4_timeout_policy_updates_for_new_loads_and_pins_old_values() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = EdgionGatewayConfigHandler::new();

        handler.partial_update(
            HashMap::from([(
                "l4-policy".to_string(),
                Arc::new(make_config_with_l4_timeouts("l4-policy", "3s", "5m")),
            )]),
            HashMap::new(),
            HashSet::new(),
        );
        let pinned = get_resolved_l4_timeouts(Some("l4-policy"));
        assert_eq!(pinned.connect_timeout, Some(Duration::from_secs(3)));
        assert_eq!(pinned.idle_timeout, Some(Duration::from_secs(300)));

        handler.partial_update(
            HashMap::new(),
            HashMap::from([(
                "l4-policy".to_string(),
                Arc::new(make_config_with_l4_timeouts("l4-policy", "0s", "0ms")),
            )]),
            HashSet::new(),
        );
        assert_eq!(
            pinned,
            ResolvedL4Timeouts {
                connect_timeout: Some(Duration::from_secs(3)),
                idle_timeout: Some(Duration::from_secs(300)),
            },
            "a connection-scoped copy must not change after publication"
        );
        assert_eq!(
            get_resolved_l4_timeouts(Some("l4-policy")),
            ResolvedL4Timeouts {
                connect_timeout: None,
                idle_timeout: None,
            },
            "a new connection load must observe the updated policy"
        );

        handler.partial_update(HashMap::new(), HashMap::new(), HashSet::from(["l4-policy".to_string()]));
        assert_eq!(
            get_resolved_l4_timeouts(Some("l4-policy")),
            ResolvedL4Timeouts::default()
        );
    }

    #[test]
    fn l4_timeout_policy_preserves_defaults_isolation_and_invalid_fallback() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = EdgionGatewayConfigHandler::new();

        handler.full_set(&HashMap::from([
            (
                "default".to_string(),
                Arc::new(make_config_with_l4_timeouts("default", "1s", "2s")),
            ),
            (
                "mixed".to_string(),
                Arc::new(make_config_with_l4_timeouts("mixed", "invalid", "0s")),
            ),
        ]));

        assert_eq!(
            get_resolved_l4_timeouts(None),
            ResolvedL4Timeouts {
                connect_timeout: Some(DEFAULT_TCP_CONNECT),
                idle_timeout: Some(DEFAULT_TCP_IDLE),
            },
            "no parametersRef must not select a real EGC named default"
        );
        assert_eq!(
            get_resolved_l4_timeouts(Some("default")),
            ResolvedL4Timeouts {
                connect_timeout: Some(Duration::from_secs(1)),
                idle_timeout: Some(Duration::from_secs(2)),
            }
        );
        assert_eq!(
            get_resolved_l4_timeouts(Some("mixed")),
            ResolvedL4Timeouts {
                connect_timeout: Some(DEFAULT_TCP_CONNECT),
                idle_timeout: None,
            },
            "each invalid field must fall back independently through its canonical resolver"
        );

        handler.full_set(&HashMap::new());
        assert_eq!(get_resolved_l4_timeouts(Some("default")), ResolvedL4Timeouts::default());
    }

    #[test]
    fn l4_timeout_accessor_does_not_retain_an_obsolete_parent_snapshot() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = EdgionGatewayConfigHandler::new();
        handler.full_set(&HashMap::from([(
            "l4-policy".to_string(),
            Arc::new(make_config_with_l4_timeouts("l4-policy", "4s", "6s")),
        )]));

        let old_snapshot = EDGION_GATEWAY_CONFIG_STORE.load_full();
        let weak = Arc::downgrade(&old_snapshot);
        assert_eq!(
            get_resolved_l4_timeouts(Some("l4-policy")).connect_timeout,
            Some(Duration::from_secs(4))
        );
        handler.full_set(&HashMap::new());
        drop(old_snapshot);

        assert!(
            weak.upgrade().is_none(),
            "the copied policy must not retain its parent snapshot"
        );
    }

    #[test]
    fn dns_resolver_default_is_precomputed_with_stable_key_winner() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = EdgionGatewayConfigHandler::new();

        let mut key_b = make_config("metadata-first");
        key_b.spec.dns_resolver = Some(DnsResolverConfig {
            link_sys_ref: None,
            servers: Some(vec!["192.0.2.2:53".to_string()]),
            cache_ttl: "20s".to_string(),
        });
        let mut key_a = make_config("metadata-second");
        key_a.spec.dns_resolver = Some(DnsResolverConfig {
            link_sys_ref: None,
            servers: Some(vec!["192.0.2.1:53".to_string()]),
            cache_ttl: "10s".to_string(),
        });
        handler.full_set(&HashMap::from([
            ("b".to_string(), Arc::new(key_b)),
            ("a".to_string(), Arc::new(key_a)),
        ]));

        let in_flight = get_egc_dns_resolver_default().expect("lowest store key must win");
        assert_eq!(
            in_flight.candidate().cloned(),
            Some(ResolverCandidate::Servers(vec!["192.0.2.1:53".to_string()]))
        );
        assert_eq!(in_flight.cache_ttl(), Duration::from_secs(10));

        let without_dns = make_config("metadata-second");
        handler.partial_update(
            HashMap::new(),
            HashMap::from([("a".to_string(), Arc::new(without_dns))]),
            HashSet::new(),
        );
        let next = get_egc_dns_resolver_default().expect("remaining resolver must become the winner");
        assert_eq!(
            next.candidate().cloned(),
            Some(ResolverCandidate::Servers(vec!["192.0.2.2:53".to_string()]))
        );
        assert_eq!(next.cache_ttl(), Duration::from_secs(20));
        assert!(!Arc::ptr_eq(&in_flight, &next));
        assert_eq!(
            in_flight.candidate().cloned(),
            Some(ResolverCandidate::Servers(vec!["192.0.2.1:53".to_string()])),
            "a loaded Arc must pin its candidate generation"
        );
        assert_eq!(
            in_flight.cache_ttl(),
            Duration::from_secs(10),
            "a loaded Arc must pin its TTL generation"
        );

        let mut invalid_system = make_config("metadata-first");
        invalid_system.spec.dns_resolver = Some(DnsResolverConfig {
            link_sys_ref: None,
            servers: None,
            cache_ttl: "invalid".to_string(),
        });
        handler.partial_update(
            HashMap::new(),
            HashMap::from([("b".to_string(), Arc::new(invalid_system))]),
            HashSet::new(),
        );
        let invalid = get_egc_dns_resolver_default().expect("configured system resolver remains represented");
        assert_eq!(invalid.candidate(), None);
        assert_eq!(invalid.cache_ttl(), crate::backends::DEFAULT_DNS_CACHE_TTL);

        let mut zero_ttl = make_config("metadata-first");
        zero_ttl.spec.dns_resolver = Some(DnsResolverConfig {
            link_sys_ref: None,
            servers: Some(vec!["192.0.2.3:53".to_string()]),
            cache_ttl: "0s".to_string(),
        });
        handler.partial_update(
            HashMap::new(),
            HashMap::from([("b".to_string(), Arc::new(zero_ttl))]),
            HashSet::new(),
        );
        assert_eq!(
            get_egc_dns_resolver_default()
                .expect("zero TTL is a valid configured generation")
                .cache_ttl(),
            Duration::ZERO
        );

        handler.partial_update(HashMap::new(), HashMap::new(), HashSet::from(["b".to_string()]));
        assert!(get_egc_dns_resolver_default().is_none());
    }

    #[test]
    fn concurrent_partial_updates_preserve_every_key() {
        use std::sync::{Arc as StdArc, Barrier};

        let _g = TEST_MUTEX.lock();
        clear_store();

        const WRITERS: usize = 16;
        let barrier = StdArc::new(Barrier::new(WRITERS + 1));
        let mut handles = Vec::with_capacity(WRITERS);
        for index in 0..WRITERS {
            let barrier = StdArc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                let name = format!("concurrent-{index}");
                let mut add = HashMap::new();
                add.insert(name.clone(), Arc::new(make_config(&name)));
                barrier.wait();
                EdgionGatewayConfigHandler::new().partial_update(add, HashMap::new(), HashSet::new());
            }));
        }

        barrier.wait();
        for handle in handles {
            handle.join().expect("concurrent policy writer must not panic");
        }

        assert_eq!(count_edgion_gateway_configs(), WRITERS);
        for index in 0..WRITERS {
            assert!(
                get_edgion_gateway_config_by_name(&format!("concurrent-{index}")).is_some(),
                "concurrent writer entry must survive"
            );
        }
    }

    #[test]
    fn test_link_sys_config_webhook_max() {
        use edgion_resources::resources::edgion_gateway_config::{EdgionGatewayConfigSpec, LinkSysGlobalConfig};
        let spec = EdgionGatewayConfigSpec {
            link_sys: Some(LinkSysGlobalConfig {
                webhook_max_response_bytes: 65536,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(spec.link_sys.unwrap().webhook_max_response_bytes, 65536);
    }

    #[test]
    fn test_list_and_count() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = EdgionGatewayConfigHandler::new();

        let mut add = HashMap::new();
        add.insert("a".to_string(), Arc::new(make_config("a")));
        add.insert("b".to_string(), Arc::new(make_config("b")));
        add.insert("c".to_string(), Arc::new(make_config("c")));
        handler.partial_update(add, HashMap::new(), std::collections::HashSet::new());

        assert_eq!(count_edgion_gateway_configs(), 3);
        assert_eq!(list_edgion_gateway_configs().len(), 3);
    }

    fn make_config_with_link_sys(name: &str, max_bytes: usize) -> EdgionGatewayConfig {
        use edgion_resources::resources::edgion_gateway_config::LinkSysGlobalConfig;
        let mut config = make_config(name);
        config.spec.link_sys = Some(LinkSysGlobalConfig {
            webhook_max_response_bytes: max_bytes,
            ..Default::default()
        });
        config
    }

    #[test]
    fn full_set_propagates_link_sys_config() {
        use crate::link_sys::providers::webhook::manager::{configure_webhook_global_max, get_webhook_global_max};
        let _g = TEST_MUTEX.lock();
        clear_store();

        // Reset the global to a known sentinel before the test
        configure_webhook_global_max(16384);
        assert_eq!(get_webhook_global_max(), 16384);

        // Construct a config with a non-default linkSys.webhookMaxResponseBytes
        let config = make_config_with_link_sys("test-config", 123_456);

        let mut data = HashMap::new();
        data.insert("default/test-config".to_string(), Arc::new(config));

        let handler = EdgionGatewayConfigHandler;
        handler.full_set(&data);

        // Global should now reflect the input
        assert_eq!(get_webhook_global_max(), 123_456);

        // Reset to default for other tests
        configure_webhook_global_max(16384);
    }

    #[test]
    fn test_load_balancing_degrade_threshold_applied() {
        use crate::backends::degrade_threshold::get_lb_degrade_default;
        use edgion_resources::resources::edgion_gateway_config::LoadBalancingConfig;

        let _g = TEST_MUTEX.lock();
        clear_store();

        let handler = EdgionGatewayConfigHandler::new();
        let mut cfg = make_config("gw-pt");
        cfg.spec.load_balancing = Some(LoadBalancingConfig { degrade_threshold: 50 });

        let mut data = HashMap::new();
        data.insert("gw-pt".to_string(), Arc::new(cfg));
        handler.full_set(&data);

        assert_eq!(get_lb_degrade_default(), 50);
        crate::backends::degrade_threshold::configure_lb_degrade_default(0);
    }

    #[test]
    fn full_set_link_sys_aggregates_min_deterministically() {
        use crate::link_sys::providers::webhook::manager::{configure_webhook_global_max, get_webhook_global_max};
        let _g = TEST_MUTEX.lock();
        clear_store();
        configure_webhook_global_max(16384);

        // Two cluster-scoped configs with different caps; the strictest (min) wins
        // regardless of HashMap iteration order.
        let mut data = HashMap::new();
        data.insert("a".to_string(), Arc::new(make_config_with_link_sys("a", 65536)));
        data.insert("b".to_string(), Arc::new(make_config_with_link_sys("b", 16384)));

        let handler = EdgionGatewayConfigHandler;
        handler.full_set(&data);
        assert_eq!(get_webhook_global_max(), 16384, "min cap must win over hash order");

        configure_webhook_global_max(16384);
    }

    #[test]
    fn remove_last_link_sys_config_resets_to_default() {
        use crate::link_sys::providers::webhook::manager::{configure_webhook_global_max, get_webhook_global_max};
        let _g = TEST_MUTEX.lock();
        clear_store();
        configure_webhook_global_max(16384);
        let handler = EdgionGatewayConfigHandler;

        // Add a single config lifting the cap to 64 KB.
        let mut add = HashMap::new();
        add.insert("cfg".to_string(), Arc::new(make_config_with_link_sys("cfg", 65536)));
        handler.partial_update(add, HashMap::new(), std::collections::HashSet::new());
        assert_eq!(get_webhook_global_max(), 65536);

        // Removing the only config carrying link_sys must restore the default,
        // not leave the relaxed cap lingering.
        let mut remove = std::collections::HashSet::new();
        remove.insert("cfg".to_string());
        handler.partial_update(HashMap::new(), HashMap::new(), remove);
        assert_eq!(get_webhook_global_max(), 16384, "delete must reset cap to default");
    }

    fn make_config_with_link_sys_instances(name: &str, max_instances: usize) -> EdgionGatewayConfig {
        use edgion_resources::resources::edgion_gateway_config::LinkSysGlobalConfig;
        let mut config = make_config(name);
        config.spec.link_sys = Some(LinkSysGlobalConfig {
            max_instances_per_kind: max_instances,
            ..Default::default()
        });
        config
    }

    #[test]
    fn full_set_link_sys_instance_cap_aggregates_min_deterministically() {
        use crate::link_sys::runtime::store::{
            configure_link_sys_max_instances_per_kind, get_link_sys_max_instances_per_kind, LINK_SYS_CAP_TEST_LOCK,
        };
        let _g = TEST_MUTEX.lock();
        let _cap = LINK_SYS_CAP_TEST_LOCK.lock();
        clear_store();
        configure_link_sys_max_instances_per_kind(200);

        let mut data = HashMap::new();
        data.insert("a".to_string(), Arc::new(make_config_with_link_sys_instances("a", 500)));
        data.insert("b".to_string(), Arc::new(make_config_with_link_sys_instances("b", 50)));

        let handler = EdgionGatewayConfigHandler;
        handler.full_set(&data);
        assert_eq!(
            get_link_sys_max_instances_per_kind(),
            50,
            "min instance cap must win over hash order"
        );

        configure_link_sys_max_instances_per_kind(200);
    }

    #[test]
    fn remove_last_link_sys_config_resets_instance_cap_to_default() {
        use crate::link_sys::runtime::store::{
            configure_link_sys_max_instances_per_kind, get_link_sys_max_instances_per_kind, LINK_SYS_CAP_TEST_LOCK,
        };
        let _g = TEST_MUTEX.lock();
        let _cap = LINK_SYS_CAP_TEST_LOCK.lock();
        clear_store();
        configure_link_sys_max_instances_per_kind(200);
        let handler = EdgionGatewayConfigHandler;

        let mut add = HashMap::new();
        add.insert(
            "cfg".to_string(),
            Arc::new(make_config_with_link_sys_instances("cfg", 50)),
        );
        handler.partial_update(add, HashMap::new(), std::collections::HashSet::new());
        assert_eq!(get_link_sys_max_instances_per_kind(), 50);

        let mut remove = std::collections::HashSet::new();
        remove.insert("cfg".to_string());
        handler.partial_update(HashMap::new(), HashMap::new(), remove);
        assert_eq!(
            get_link_sys_max_instances_per_kind(),
            200,
            "delete must reset instance cap to default"
        );
    }

    #[test]
    fn load_balancing_degrade_threshold_aggregates_max_and_resets() {
        use crate::backends::degrade_threshold::{configure_lb_degrade_default, get_lb_degrade_default};
        use edgion_resources::resources::edgion_gateway_config::LoadBalancingConfig;
        let _g = TEST_MUTEX.lock();
        clear_store();
        configure_lb_degrade_default(0);
        let handler = EdgionGatewayConfigHandler;

        let mut lo = make_config("lo");
        lo.spec.load_balancing = Some(LoadBalancingConfig { degrade_threshold: 30 });
        let mut hi = make_config("hi");
        hi.spec.load_balancing = Some(LoadBalancingConfig { degrade_threshold: 70 });

        let mut data = HashMap::new();
        data.insert("lo".to_string(), Arc::new(lo));
        data.insert("hi".to_string(), Arc::new(hi));
        handler.full_set(&data);
        assert_eq!(get_lb_degrade_default(), 70, "max threshold must win over hash order");

        // Drop back to a single config; threshold follows the survivor.
        let mut only_lo = make_config("lo");
        only_lo.spec.load_balancing = Some(LoadBalancingConfig { degrade_threshold: 30 });
        let mut data2 = HashMap::new();
        data2.insert("lo".to_string(), Arc::new(only_lo));
        handler.full_set(&data2);
        assert_eq!(get_lb_degrade_default(), 30);

        // Empty store resets to the disabled default.
        handler.full_set(&HashMap::new());
        assert_eq!(
            get_lb_degrade_default(),
            0,
            "no load_balancing config must reset to disabled"
        );

        configure_lb_degrade_default(0);
    }

    #[test]
    fn load_balancing_out_of_range_threshold_does_not_poison_max() {
        use crate::backends::degrade_threshold::{configure_lb_degrade_default, get_lb_degrade_default};
        use edgion_resources::resources::edgion_gateway_config::LoadBalancingConfig;
        let _g = TEST_MUTEX.lock();
        clear_store();
        configure_lb_degrade_default(0);
        let handler = EdgionGatewayConfigHandler;

        // One config is out of range (>100); it must be ignored rather than win the
        // max and clamp the whole gateway to 0 (disabled).
        let mut good = make_config("good");
        good.spec.load_balancing = Some(LoadBalancingConfig { degrade_threshold: 70 });
        let mut bad = make_config("bad");
        bad.spec.load_balancing = Some(LoadBalancingConfig { degrade_threshold: 150 });

        let mut data = HashMap::new();
        data.insert("good".to_string(), Arc::new(good));
        data.insert("bad".to_string(), Arc::new(bad));
        handler.full_set(&data);
        assert_eq!(
            get_lb_degrade_default(),
            70,
            "out-of-range threshold must not poison the max"
        );

        configure_lb_degrade_default(0);
    }

    #[test]
    fn outbound_tls_safest_wins_and_resets() {
        use edgion_common::link_sys_core::outbound_tls::{outbound_global_default, set_outbound_global_default};
        use edgion_resources::resources::common::OutboundTlsConfig;
        let _g = TEST_MUTEX.lock();
        clear_store();
        // Reset global default to known state
        set_outbound_global_default(OutboundTlsConfig::default());

        let handler = EdgionGatewayConfigHandler;

        // One config with verify=false, one with no outbound_tls (verify defaults true)
        let mut cfg_a = make_config("a");
        cfg_a.spec.outbound_tls = Some(OutboundTlsConfig {
            verify: false,
            ..OutboundTlsConfig::default()
        });
        let cfg_b = make_config("b"); // outbound_tls: None -> verify=true by default

        let mut data = HashMap::new();
        data.insert("a".to_string(), Arc::new(cfg_a));
        data.insert("b".to_string(), Arc::new(cfg_b));
        handler.full_set(&data);
        // safest-wins: one block is absent (defaults true) → aggregate is true
        assert!(
            outbound_global_default().verify,
            "one absent block must keep verify=true (safest-wins)"
        );

        // Both verify=false → aggregate is false
        let mut cfg_c = make_config("c");
        cfg_c.spec.outbound_tls = Some(OutboundTlsConfig {
            verify: false,
            ..OutboundTlsConfig::default()
        });
        let mut cfg_d = make_config("d");
        cfg_d.spec.outbound_tls = Some(OutboundTlsConfig {
            verify: false,
            ..OutboundTlsConfig::default()
        });
        let mut data2 = HashMap::new();
        data2.insert("c".to_string(), Arc::new(cfg_c));
        data2.insert("d".to_string(), Arc::new(cfg_d));
        handler.full_set(&data2);
        assert!(
            !outbound_global_default().verify,
            "both verify=false must aggregate to false"
        );

        // Reset to default for subsequent tests
        set_outbound_global_default(OutboundTlsConfig::default());
        clear_store();
    }

    #[test]
    fn outbound_tls_adopts_mtls_only_global_default() {
        use edgion_common::link_sys_core::outbound_tls::{outbound_global_default, set_outbound_global_default};
        use edgion_resources::resources::common::{OutboundTlsConfig, SecretObjectReference};
        use k8s_openapi::api::core::v1::Secret;
        use k8s_openapi::ByteString;
        use std::collections::BTreeMap;

        let _g = TEST_MUTEX.lock();
        clear_store();
        set_outbound_global_default(OutboundTlsConfig::default());
        let identity = rcgen::generate_simple_self_signed(vec!["client.example.com".to_string()])
            .expect("generate test client identity");
        let mut data_map = BTreeMap::new();
        data_map.insert("tls.crt".to_string(), ByteString(identity.cert.pem().into_bytes()));
        data_map.insert(
            "tls.key".to_string(),
            ByteString(identity.signing_key.serialize_pem().into_bytes()),
        );

        let mut config = make_config("mtls-only");
        config.spec.outbound_tls = Some(OutboundTlsConfig {
            client_certificate_ref: Some(SecretObjectReference {
                group: None,
                kind: None,
                name: "global-client".to_string(),
                namespace: None,
            }),
            resolved_client_certificate: Some(Secret {
                data: Some(data_map),
                ..Default::default()
            }),
            ..Default::default()
        });

        let handler = EdgionGatewayConfigHandler;
        handler.full_set(&HashMap::from([("mtls-only".to_string(), Arc::new(config))]));
        let global = outbound_global_default();
        set_outbound_global_default(OutboundTlsConfig::default());
        clear_store();
        assert!(global.client_certificate_ref.is_some());
        assert!(global.resolved_client_certificate.is_some());
    }

    #[test]
    fn outbound_tls_conflicting_global_mtls_defaults_fail_closed() {
        use edgion_common::link_sys_core::outbound_tls::{outbound_global_default, set_outbound_global_default};
        use edgion_resources::resources::common::{OutboundTlsConfig, SecretObjectReference};

        let _g = TEST_MUTEX.lock();
        clear_store();
        set_outbound_global_default(OutboundTlsConfig::default());

        let mut first = make_config("first");
        first.spec.outbound_tls = Some(OutboundTlsConfig {
            client_certificate_ref: Some(SecretObjectReference {
                group: None,
                kind: None,
                name: "client-a".to_string(),
                namespace: None,
            }),
            ..Default::default()
        });
        let mut second = make_config("second");
        second.spec.outbound_tls = Some(OutboundTlsConfig {
            client_certificate_ref: Some(SecretObjectReference {
                group: None,
                kind: None,
                name: "client-b".to_string(),
                namespace: None,
            }),
            ..Default::default()
        });

        let handler = EdgionGatewayConfigHandler;
        handler.full_set(&HashMap::from([
            ("first".to_string(), Arc::new(first)),
            ("second".to_string(), Arc::new(second)),
        ]));
        let global = outbound_global_default();
        set_outbound_global_default(OutboundTlsConfig::default());
        clear_store();
        assert!(global.client_certificate_ref.is_some());
        assert!(global.resolved_client_certificate.is_none());
    }

    #[test]
    fn partial_update_propagates_link_sys_config_on_add_and_update() {
        use crate::link_sys::providers::webhook::manager::{configure_webhook_global_max, get_webhook_global_max};
        let _g = TEST_MUTEX.lock();
        clear_store();

        configure_webhook_global_max(16384);
        let handler = EdgionGatewayConfigHandler;

        // ADD path
        let cfg_a = make_config_with_link_sys("cfg-a", 33_333);
        let mut add = HashMap::new();
        add.insert("default/cfg-a".to_string(), Arc::new(cfg_a));
        handler.partial_update(add, HashMap::new(), std::collections::HashSet::new());
        assert_eq!(get_webhook_global_max(), 33_333);

        // UPDATE path
        let cfg_b = make_config_with_link_sys("cfg-a", 44_444);
        let mut upd = HashMap::new();
        upd.insert("default/cfg-a".to_string(), Arc::new(cfg_b));
        handler.partial_update(HashMap::new(), upd, std::collections::HashSet::new());
        assert_eq!(get_webhook_global_max(), 44_444);

        // Reset
        configure_webhook_global_max(16384);
    }

    // ------------------------------------------------------------------
    // M2: a single config with validation.caCertificateRefs + resolved_ca_certificates
    // is adopted as the cluster-wide default.
    // ------------------------------------------------------------------

    #[test]
    fn aggregate_adopts_single_validation_block() {
        use edgion_common::link_sys_core::outbound_tls::{outbound_global_default, set_outbound_global_default};
        use edgion_resources::resources::backend_tls_policy::BackendTLSPolicyCACertificateRef;
        use edgion_resources::resources::common::{outbound_tls::OutboundTlsValidation, OutboundTlsConfig};
        use k8s_openapi::api::core::v1::Secret;
        use k8s_openapi::ByteString;
        let _g = TEST_MUTEX.lock();
        clear_store();
        set_outbound_global_default(OutboundTlsConfig::default());

        let handler = EdgionGatewayConfigHandler;

        let mut data_map = std::collections::BTreeMap::new();
        data_map.insert(
            "ca.crt".to_string(),
            ByteString(b"-----BEGIN CERTIFICATE-----\nMIIBglobal\n-----END CERTIFICATE-----\n".to_vec()),
        );
        let resolved = Secret {
            data: Some(data_map),
            type_: Some("kubernetes.io/tls".to_string()),
            ..Default::default()
        };

        let mut cfg = make_config("egc-validation");
        cfg.spec.outbound_tls = Some(OutboundTlsConfig {
            verify: true,
            validation: Some(OutboundTlsValidation {
                ca_certificate_refs: Some(vec![BackendTLSPolicyCACertificateRef {
                    kind: "Secret".to_string(),
                    name: "my-ca-secret".to_string(),
                    group: String::new(),
                    namespace: None,
                }]),
                well_known_ca_certificates: None,
                hostname: None,
                subject_alt_names: None,
            }),
            resolved_ca_certificates: Some(vec![resolved]),
            client_certificate_ref: None,
            resolved_client_certificate: None,
        });

        let mut data = HashMap::new();
        data.insert("egc-validation".to_string(), Arc::new(cfg));
        handler.full_set(&data);

        // The single validation with caCertificateRefs is adopted as the cluster-wide
        // default, with the resolved CA certificates carried through.
        let global = outbound_global_default();
        assert!(global.validation.is_some(), "global validation must be adopted");
        assert!(
            global.validation.as_ref().unwrap().ca_certificate_refs.is_some(),
            "ca_certificate_refs must be in the validation"
        );
        assert!(
            global.resolved_ca_certificates.is_some(),
            "resolved_ca_certificates must be carried into the global default"
        );

        set_outbound_global_default(OutboundTlsConfig::default());
    }

    // ------------------------------------------------------------------
    // M-C3: outbound TLS aggregation — delete-path restore + CA adopt/conflict
    // ------------------------------------------------------------------

    #[test]
    fn outbound_tls_delete_path_restores_verify_default() {
        use edgion_common::link_sys_core::outbound_tls::{outbound_global_default, set_outbound_global_default};
        use edgion_resources::resources::common::OutboundTlsConfig;
        let _g = TEST_MUTEX.lock();
        clear_store();
        set_outbound_global_default(OutboundTlsConfig::default());

        let handler = EdgionGatewayConfigHandler;

        // full_set: one EGC verify=false, one absent (defaults true) → safest-wins → true
        let mut cfg_a = make_config("a");
        cfg_a.spec.outbound_tls = Some(OutboundTlsConfig {
            verify: false,
            ..OutboundTlsConfig::default()
        });
        let cfg_b = make_config("b"); // outbound_tls: None → verify defaults true
        let mut data = HashMap::new();
        data.insert("a".to_string(), Arc::new(cfg_a));
        data.insert("b".to_string(), Arc::new(cfg_b));
        handler.full_set(&data);
        assert!(
            outbound_global_default().verify,
            "safest-wins: one absent block must keep verify=true"
        );

        // partial_update removing all entries → default restored
        let mut remove = std::collections::HashSet::new();
        remove.insert("a".to_string());
        remove.insert("b".to_string());
        handler.partial_update(HashMap::new(), HashMap::new(), remove);
        assert!(
            outbound_global_default().verify,
            "after removing all configs, verify must reset to default (true)"
        );

        set_outbound_global_default(OutboundTlsConfig::default());
    }

    #[test]
    fn aggregate_conflicts_fallback_to_system() {
        use edgion_common::link_sys_core::outbound_tls::{outbound_global_default, set_outbound_global_default};
        use edgion_resources::resources::backend_tls_policy::BackendTLSPolicyCACertificateRef;
        use edgion_resources::resources::common::{outbound_tls::OutboundTlsValidation, OutboundTlsConfig};
        let _g = TEST_MUTEX.lock();
        clear_store();
        set_outbound_global_default(OutboundTlsConfig::default());

        let handler = EdgionGatewayConfigHandler;

        let mut cfg_a = make_config("ca-a");
        cfg_a.spec.outbound_tls = Some(OutboundTlsConfig {
            verify: true,
            validation: Some(OutboundTlsValidation {
                ca_certificate_refs: Some(vec![BackendTLSPolicyCACertificateRef {
                    kind: "Secret".to_string(),
                    name: "ca-a-secret".to_string(),
                    group: String::new(),
                    namespace: None,
                }]),
                well_known_ca_certificates: None,
                hostname: None,
                subject_alt_names: None,
            }),
            resolved_ca_certificates: None,
            client_certificate_ref: None,
            resolved_client_certificate: None,
        });
        let mut cfg_b = make_config("ca-b");
        cfg_b.spec.outbound_tls = Some(OutboundTlsConfig {
            verify: true,
            validation: Some(OutboundTlsValidation {
                ca_certificate_refs: Some(vec![BackendTLSPolicyCACertificateRef {
                    kind: "Secret".to_string(),
                    name: "ca-b-secret".to_string(),
                    group: String::new(),
                    namespace: None,
                }]),
                well_known_ca_certificates: None,
                hostname: None,
                subject_alt_names: None,
            }),
            resolved_ca_certificates: None,
            client_certificate_ref: None,
            resolved_client_certificate: None,
        });

        let mut data = HashMap::new();
        data.insert("ca-a".to_string(), Arc::new(cfg_a));
        data.insert("ca-b".to_string(), Arc::new(cfg_b));
        handler.full_set(&data);

        let global = outbound_global_default();
        assert!(
            global.validation.is_none(),
            "conflicting validations must fall back to None (System), got: {:?}",
            global.validation
        );

        set_outbound_global_default(OutboundTlsConfig::default());
    }
}
