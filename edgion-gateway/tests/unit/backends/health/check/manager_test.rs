use super::*;

#[tokio::test]
async fn reconciliation_owns_status_marker_lifecycle() {
    let health_store = Arc::new(HealthStatusStore::new());
    let config_store = Arc::new(HealthCheckConfigStore::new());
    let manager = HealthCheckManager {
        tasks: RwLock::new(HashMap::new()),
        health_store: Arc::clone(&health_store),
        config_store: Arc::clone(&config_store),
        reconcile_lock: Mutex::new(()),
    };
    let service_key = "health-status-owner/service";

    config_store.set_service_config(service_key, Some(test_owner(ActiveHealthCheckConfig::default())));
    manager.reconcile_service(service_key);
    assert!(health_store.has_service(service_key));

    let old_task = manager.tasks.read()[service_key].abort_handle();
    manager.reconcile_service(service_key);
    tokio::task::yield_now().await;
    assert!(old_task.is_finished());
    assert_eq!(health_store.tracked_service_count(), 1);

    config_store.remove_service(service_key);
    manager.reconcile_service(service_key);
    tokio::task::yield_now().await;

    assert!(!health_store.has_service(service_key));
    assert!(!manager.tasks.read().contains_key(service_key));
    assert_eq!(health_store.tracked_service_count(), 0);
    assert!(health_store.keys().is_empty());
}

#[test]
fn reconciliation_without_runtime_cancels_task_and_clears_marker() {
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    let health_store = Arc::new(HealthStatusStore::new());
    let config_store = Arc::new(HealthCheckConfigStore::new());
    let manager = HealthCheckManager {
        tasks: RwLock::new(HashMap::new()),
        health_store: Arc::clone(&health_store),
        config_store: Arc::clone(&config_store),
        reconcile_lock: Mutex::new(()),
    };
    let service_key = "health-status-no-runtime/service";
    let task = runtime.spawn(std::future::pending::<()>());
    let abort = task.abort_handle();
    manager.tasks.write().insert(service_key.to_string(), task);
    health_store.register_service(service_key);
    config_store.set_service_config(service_key, Some(test_owner(ActiveHealthCheckConfig::default())));
    manager.reconcile_service(service_key);
    assert!(!manager.tasks.read().contains_key(service_key));
    assert_eq!(health_store.tracked_service_count(), 0);
    assert!(health_store.keys().is_empty());
    runtime.block_on(tokio::task::yield_now());
    assert!(abort.is_finished());
    config_store.remove_service(service_key);
    manager.reconcile_service(service_key);
}

#[test]
fn test_derive_startup_jitter_within_bound() {
    let max = Duration::from_millis(500);
    let jitter = derive_startup_jitter("default/svc-a", max);
    assert!(jitter <= max);
}

#[test]
fn test_derive_startup_jitter_deterministic() {
    let max = Duration::from_secs(3);
    let a = derive_startup_jitter("default/svc-a", max);
    let b = derive_startup_jitter("default/svc-a", max);
    assert_eq!(a, b);
}

#[test]
fn test_derive_startup_jitter_zero_bound() {
    let jitter = derive_startup_jitter("default/svc-a", Duration::ZERO);
    assert_eq!(jitter, Duration::ZERO);
}

mod resolve_probe_host_tests {
    use super::*;
    use crate::backends::policy::backend_traffic::store::GLOBAL_STORE_TEST_LOCK;
    use edgion_resources::resources::edgion_backend_traffic_policy::{EdgionBackendTrafficPolicySpec, PolicyTargetRef};
    use edgion_resources::resources::upstream_authority::UpstreamAuthorityConfig;
    use edgion_resources::resources::EdgionBackendTrafficPolicy;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use std::collections::{HashMap, HashSet};

    /// Build a minimal valid `EdgionBackendTrafficPolicy` targeting `svc`
    /// (within namespace `ns`) with the given `upstreamAuthority`.
    fn policy_with_authority(
        ns: &str,
        name: &str,
        svc: &str,
        authority: Option<UpstreamAuthorityConfig>,
    ) -> Arc<EdgionBackendTrafficPolicy> {
        Arc::new(EdgionBackendTrafficPolicy {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(ns.to_string()),
                ..Default::default()
            },
            spec: EdgionBackendTrafficPolicySpec {
            prepared_ancestor_admission: Some(edgion_resources::resources::common::policy_ancestor_admission::PreparedPolicyAncestorAdmission::Unrestricted),
                prepared_precedence: Some(
                    edgion_resources::resources::common::policy_precedence::PreparedPolicyPrecedence {
                        creation_timestamp: None,
                    },
                ),
                current_status: None,
                target_refs: vec![PolicyTargetRef {
                    group: Some(String::new()),
                    kind: "Service".to_string(),
                    name: svc.to_string(),
                }],
                resolved_target_class: Some(
                    edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass::Service,
                ),
                retry_constraint: None,
                load_balancer: None,
                health_check: None,
                outlier_detection: None,
                upstream_authority: authority,
                circuit_breaker: None,
                connection: None,
            },
            status: None,
        })
    }

    /// Insert `policy` into the process-global `BackendTrafficPolicyStore`
    /// under `policy_key` and reapply, then hand back a guard that removes
    /// it again on drop so other tests sharing the singleton are unaffected.
    struct InstalledPolicy {
        policy_key: String,
        service_key: String,
    }

    impl InstalledPolicy {
        fn install(policy_key: &str, service_key: &str, policy: Arc<EdgionBackendTrafficPolicy>) -> Self {
            let mut add = HashMap::new();
            add.insert(policy_key.to_string(), policy);
            get_backend_traffic_policy_store().partial_update_policies(add, HashMap::new(), HashSet::new());
            Self {
                policy_key: policy_key.to_string(),
                service_key: service_key.to_string(),
            }
        }
    }

    impl Drop for InstalledPolicy {
        fn drop(&mut self) {
            let mut remove = HashSet::new();
            remove.insert(self.policy_key.clone());
            get_backend_traffic_policy_store().partial_update_policies(HashMap::new(), HashMap::new(), remove);
            // Sanity: the authority must be gone after cleanup so later tests
            // (or this one, if the assertion above already failed) never see
            // a stale entry for this key.
            debug_assert!(get_backend_traffic_policy_store()
                .get_authority(&self.service_key)
                .is_none());
        }
    }

    // `healthCheckHost` present on the resolved authority wins over a
    // different `config_host`. This is the precedence the report describes,
    // and the one the moved authority-write fix (store.rs) exists to serve
    // fresh to the spawned probe task.
    #[test]
    fn health_check_host_overrides_config_host() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let ns = "hcresolve-a";
        let svc = "svc";
        let service_key = format!("{ns}/{svc}");
        let policy_key = format!("{ns}/p1");

        let policy = policy_with_authority(
            ns,
            "p1",
            svc,
            Some(UpstreamAuthorityConfig {
                pattern: "*.sandbox.internal".to_string(),
                template: "${ctx:tenant}.sandbox.internal".to_string(),
                health_check_host: Some("probe.sandbox.internal".to_string()),
            }),
        );
        let _installed = InstalledPolicy::install(&policy_key, &service_key, policy);

        let resolved = resolve_probe_host(&service_key, Some("configured-host"));
        assert_eq!(resolved, Some("probe.sandbox.internal".to_string()));
    }

    #[test]
    fn publish_then_probe_sees_new_host() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let ns = "hcresolve-d";
        let svc = "svc";
        let service_key = format!("{ns}/{svc}");
        let policy_key = format!("{ns}/p1");

        let first = policy_with_authority(
            ns,
            "p1",
            svc,
            Some(UpstreamAuthorityConfig {
                pattern: "*.sandbox.internal".to_string(),
                template: "${ctx:tenant}.sandbox.internal".to_string(),
                health_check_host: Some("probe-old.sandbox.internal".to_string()),
            }),
        );
        let _installed = InstalledPolicy::install(&policy_key, &service_key, first);
        assert_eq!(
            resolve_probe_host(&service_key, Some("configured-host")),
            Some("probe-old.sandbox.internal".to_string())
        );

        let second = policy_with_authority(
            ns,
            "p1",
            svc,
            Some(UpstreamAuthorityConfig {
                pattern: "*.sandbox.internal".to_string(),
                template: "${ctx:tenant}.sandbox.internal".to_string(),
                health_check_host: Some("probe-new.sandbox.internal".to_string()),
            }),
        );
        let mut update = HashMap::new();
        update.insert(policy_key, second);
        get_backend_traffic_policy_store().partial_update_policies(HashMap::new(), update, HashSet::new());
        assert_eq!(
            resolve_probe_host(&service_key, Some("configured-host")),
            Some("probe-new.sandbox.internal".to_string())
        );
    }

    // No policy attached to the Service: the helper falls back to the
    // configured static host unchanged.
    #[test]
    fn no_policy_falls_back_to_config_host() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let service_key = "hcresolve-b/svc";

        let resolved = resolve_probe_host(service_key, Some("configured-host"));
        assert_eq!(resolved, Some("configured-host".to_string()));
    }

    // A policy is attached, but its authority carries no `healthCheckHost`:
    // the helper still falls back to the configured static host.
    #[test]
    fn authority_without_health_check_host_falls_back_to_config_host() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let ns = "hcresolve-c";
        let svc = "svc";
        let service_key = format!("{ns}/{svc}");
        let policy_key = format!("{ns}/p1");

        let policy = policy_with_authority(
            ns,
            "p1",
            svc,
            Some(UpstreamAuthorityConfig {
                pattern: "*.sandbox.internal".to_string(),
                template: "${ctx:tenant}.sandbox.internal".to_string(),
                health_check_host: None,
            }),
        );
        let _installed = InstalledPolicy::install(&policy_key, &service_key, policy);

        let resolved = resolve_probe_host(&service_key, Some("configured-host"));
        assert_eq!(resolved, Some("configured-host".to_string()));
    }
}

fn test_owner(config: ActiveHealthCheckConfig) -> Arc<edgion_resources::EdgionBackendTrafficPolicy> {
    let mut owner: edgion_resources::EdgionBackendTrafficPolicy = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "health", "namespace": "default"},
        "spec": {"targetRefs": [{"kind": "Service", "name": "svc"}]}
    }))
    .unwrap();
    owner.spec.health_check =
        Some(edgion_resources::resources::health_check::ServiceHealthCheck { active: Some(config) });
    Arc::new(owner)
}

#[test]
fn dns_derived_unsafe_endpoints_are_never_probed() {
    for unsafe_addr in [
        "127.0.0.1:80",
        "[::1]:80",
        "[::ffff:127.0.0.1]:80",
        "169.254.169.254:80",
    ] {
        assert!(refuses_dns_endpoint(unsafe_addr.parse().unwrap()), "{unsafe_addr}");
    }
    assert!(!refuses_dns_endpoint("10.0.0.1:80".parse().unwrap()));
    assert!(!refuses_dns_endpoint("[2001:db8::1]:80".parse().unwrap()));
}
