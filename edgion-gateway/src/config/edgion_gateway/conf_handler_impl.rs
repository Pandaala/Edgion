use crate::link_sys::providers::webhook::manager::configure_webhook_global_max;
use edgion_common::conf_sync::traits::{arc_map_to_owned, ConfHandler};
use edgion_resources::prelude_resources::EdgionGatewayConfig;
use kube::ResourceExt;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Global store for EdgionGatewayConfig resources (keyed by key_name())
static EDGION_GATEWAY_CONFIG_STORE: std::sync::LazyLock<RwLock<HashMap<String, EdgionGatewayConfig>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

/// Count EdgionGatewayConfig resources in the store
pub fn count_edgion_gateway_configs() -> usize {
    EDGION_GATEWAY_CONFIG_STORE.read().len()
}

/// Query EdgionGatewayConfig by name (cluster-scoped: key == name)
#[allow(dead_code)]
pub fn get_edgion_gateway_config_by_name(name: &str) -> Option<EdgionGatewayConfig> {
    EDGION_GATEWAY_CONFIG_STORE.read().get(name).cloned()
}

/// List all EdgionGatewayConfig resources
#[allow(dead_code)]
pub fn list_edgion_gateway_configs() -> Vec<EdgionGatewayConfig> {
    let mut items: Vec<EdgionGatewayConfig> = EDGION_GATEWAY_CONFIG_STORE.read().values().cloned().collect();
    items.sort_by(|a, b| a.name_any().cmp(&b.name_any()));
    items
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

/// Default LB panic threshold when no `EdgionGatewayConfig` sets `load_balancing`.
/// Must mirror `GLOBAL_DEFAULT`'s initial value (`panic_threshold.rs`): 0 = panic
/// mode disabled (opt-in).
const DEFAULT_LB_PANIC_THRESHOLD: u8 = 0;

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
///   - LB panic threshold:   `max` — panic mode engages as early as any config asks.
fn recompute_global_config(store: &HashMap<String, EdgionGatewayConfig>) {
    let webhook_max = store
        .values()
        .filter_map(|config| config.spec.link_sys.as_ref())
        .map(|link_sys| link_sys.webhook_max_response_bytes)
        .min()
        .unwrap_or(DEFAULT_WEBHOOK_MAX_RESPONSE_BYTES);
    configure_webhook_global_max(webhook_max);

    // Drop out-of-range thresholds before aggregating so one invalid config
    // (`> 100`) cannot poison the `max` and silently disable panic mode for every
    // service — mirrors how the per-service annotation parser ignores invalid
    // values and falls back. If all configs are invalid the aggregate is empty and
    // we reset to the default.
    let panic_threshold = store
        .values()
        .filter_map(|config| config.spec.load_balancing.as_ref())
        .map(|lb| lb.panic_threshold)
        .filter(|&threshold| threshold <= 100)
        .max()
        .unwrap_or(DEFAULT_LB_PANIC_THRESHOLD);
    crate::backends::panic_threshold::configure_lb_panic_default(panic_threshold);

    // Aggregate the cluster-wide outbound TLS default. Safest-wins on verify;
    // CA material (validation + resolved bundles + clientCertificateRef) is adopted only when exactly one config declares non-empty validation (refs or wellKnown).
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
            agg.client_certificate_ref = one.client_certificate_ref.clone();
            agg.resolved_client_certificate = one.resolved_client_certificate.clone();
        }
        [] => {}
        _ => {
            tracing::warn!(
                component = "outbound_tls",
                "multiple EdgionGatewayConfig declare conflicting outbound CA defaults; falling back to System"
            );
        }
    }
    edgion_common::link_sys_core::outbound_tls::set_outbound_global_default(agg);
}

impl ConfHandler<EdgionGatewayConfig> for EdgionGatewayConfigHandler {
    fn full_set(&self, data: &HashMap<String, Arc<EdgionGatewayConfig>>) {
        tracing::info!(
            count = data.len(),
            "EdgionGatewayConfig full_set: received {} EdgionGatewayConfig resources",
            data.len()
        );
        let mut store = EDGION_GATEWAY_CONFIG_STORE.write();
        *store = arc_map_to_owned(data);
        recompute_global_config(&store);
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<EdgionGatewayConfig>>,
        update: HashMap<String, Arc<EdgionGatewayConfig>>,
        remove: HashSet<String>,
    ) {
        let mut store = EDGION_GATEWAY_CONFIG_STORE.write();

        if !add.is_empty() {
            tracing::info!(
                count = add.len(),
                "EdgionGatewayConfig partial_update: added {} EdgionGatewayConfig resources",
                add.len()
            );
            for (key, config) in add {
                tracing::debug!(key = %key, name = %config.name_any(), "EdgionGatewayConfig added");
                store.insert(key, (*config).clone());
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
                if store.insert(key.clone(), (*config).clone()).is_none() {
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
                if store.remove(key).is_none() {
                    tracing::debug!(key = %key, "EdgionGatewayConfig remove: key not found in store");
                }
            }
        }

        // Recompute the process-global values from the post-mutation store so the
        // result is deterministic across instances and the delete path restores
        // the default once no config carries the field.
        recompute_global_config(&store);
    }
}

/// Create EdgionGatewayConfig handler
pub fn create_edgion_gateway_config_handler() -> Box<dyn ConfHandler<EdgionGatewayConfig> + Send + Sync> {
    Box::new(EdgionGatewayConfigHandler::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::edgion_gateway_config::EdgionGatewayConfigSpec;
    use std::collections::HashMap;

    // Serialize tests to avoid global-store interference
    static TEST_MUTEX: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    fn make_config(name: &str) -> EdgionGatewayConfig {
        EdgionGatewayConfig::new(
            name,
            EdgionGatewayConfigSpec {
                server: None,
                http_timeout: None,
                max_retries: 3,
                real_ip: None,
                security_protect: None,
                global_plugins_ref: None,
                preflight_policy: None,
                enable_reference_grant_validation: false,
                link_sys: None,
                tcp_timeout: None,
                load_balancing: None,
                outbound_tls: None,
            },
        )
    }

    fn clear_store() {
        EDGION_GATEWAY_CONFIG_STORE.write().clear();
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
    fn test_link_sys_config_webhook_max() {
        use edgion_resources::resources::edgion_gateway_config::{EdgionGatewayConfigSpec, LinkSysGlobalConfig};
        let spec = EdgionGatewayConfigSpec {
            link_sys: Some(LinkSysGlobalConfig {
                webhook_max_response_bytes: 65536,
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
    fn test_load_balancing_panic_threshold_applied() {
        use crate::backends::panic_threshold::get_lb_panic_default;
        use edgion_resources::resources::edgion_gateway_config::LoadBalancingConfig;

        let _g = TEST_MUTEX.lock();
        clear_store();

        let handler = EdgionGatewayConfigHandler::new();
        let mut cfg = make_config("gw-pt");
        cfg.spec.load_balancing = Some(LoadBalancingConfig { panic_threshold: 50 });

        let mut data = HashMap::new();
        data.insert("gw-pt".to_string(), Arc::new(cfg));
        handler.full_set(&data);

        assert_eq!(get_lb_panic_default(), 50);
        crate::backends::panic_threshold::configure_lb_panic_default(0);
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

    #[test]
    fn load_balancing_panic_threshold_aggregates_max_and_resets() {
        use crate::backends::panic_threshold::{configure_lb_panic_default, get_lb_panic_default};
        use edgion_resources::resources::edgion_gateway_config::LoadBalancingConfig;
        let _g = TEST_MUTEX.lock();
        clear_store();
        configure_lb_panic_default(0);
        let handler = EdgionGatewayConfigHandler;

        let mut lo = make_config("lo");
        lo.spec.load_balancing = Some(LoadBalancingConfig { panic_threshold: 30 });
        let mut hi = make_config("hi");
        hi.spec.load_balancing = Some(LoadBalancingConfig { panic_threshold: 70 });

        let mut data = HashMap::new();
        data.insert("lo".to_string(), Arc::new(lo));
        data.insert("hi".to_string(), Arc::new(hi));
        handler.full_set(&data);
        assert_eq!(get_lb_panic_default(), 70, "max threshold must win over hash order");

        // Drop back to a single config; threshold follows the survivor.
        let mut only_lo = make_config("lo");
        only_lo.spec.load_balancing = Some(LoadBalancingConfig { panic_threshold: 30 });
        let mut data2 = HashMap::new();
        data2.insert("lo".to_string(), Arc::new(only_lo));
        handler.full_set(&data2);
        assert_eq!(get_lb_panic_default(), 30);

        // Empty store resets to the disabled default.
        handler.full_set(&HashMap::new());
        assert_eq!(
            get_lb_panic_default(),
            0,
            "no load_balancing config must reset to disabled"
        );

        configure_lb_panic_default(0);
    }

    #[test]
    fn load_balancing_out_of_range_threshold_does_not_poison_max() {
        use crate::backends::panic_threshold::{configure_lb_panic_default, get_lb_panic_default};
        use edgion_resources::resources::edgion_gateway_config::LoadBalancingConfig;
        let _g = TEST_MUTEX.lock();
        clear_store();
        configure_lb_panic_default(0);
        let handler = EdgionGatewayConfigHandler;

        // One config is out of range (>100); it must be ignored rather than win the
        // max and clamp the whole gateway to 0 (disabled).
        let mut good = make_config("good");
        good.spec.load_balancing = Some(LoadBalancingConfig { panic_threshold: 70 });
        let mut bad = make_config("bad");
        bad.spec.load_balancing = Some(LoadBalancingConfig { panic_threshold: 150 });

        let mut data = HashMap::new();
        data.insert("good".to_string(), Arc::new(good));
        data.insert("bad".to_string(), Arc::new(bad));
        handler.full_set(&data);
        assert_eq!(
            get_lb_panic_default(),
            70,
            "out-of-range threshold must not poison the max"
        );

        configure_lb_panic_default(0);
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
