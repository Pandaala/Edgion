use super::*;
use edgion_resources::resources::edgion_plugins::{AiBackendSelectionConfig, AiProxyConfig, OnUnknownModel};
use edgion_resources::resources::http_route::HTTPRouteFilterType;

fn key(route_uid: &str, model: &str) -> AdaptiveTelemetryKey {
    AdaptiveTelemetryKey::new(
        route_uid.to_string(),
        "rule-a".to_string(),
        PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionBackend,
            namespace: "default".to_string(),
            name: "provider".to_string(),
        },
        model.to_string(),
    )
    .unwrap()
}

fn adaptive_entry(selection_type: AiBackendSelection, enable: bool) -> RequestFilterEntry {
    RequestFilterEntry::with_enable(
        EdgionPlugin::AiProxy(AiProxyConfig {
            api: edgion_resources::resources::edgion_plugins::AiApi::OpenaiChat,
            conversion: None,
            usage_observation: edgion_resources::resources::edgion_plugins::AiUsageObservation::Provider,
            backend_selection: AiBackendSelectionConfig { selection_type },
            inject_usage_tracking: true,
            on_unknown_model: OnUnknownModel::Reject,
            model_routes: Vec::new(),
            token_quota: None,
        }),
        enable,
    )
}

fn adaptive_route() -> HTTPRoute {
    serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "HTTPRoute",
        "metadata": {
            "name": "adaptive-route",
            "namespace": "route-ns"
        },
        "spec": {
            "rules": [{
                "name": "primary",
                "filters": [{
                    "type": "ExtensionRef",
                    "extensionRef": {
                        "group": "edgion.io",
                        "kind": "EdgionPlugins",
                        "name": "adaptive"
                    }
                }],
                "backendRefs": [{
                    "group": "edgion.io",
                    "kind": "EdgionBackend",
                    "name": "provider"
                }],
                "resolvedTerminalRouteUid": "route-uid",
                "resolvedTerminalRuleIdentity": "name:primary"
            }]
        }
    }))
    .expect("adaptive route fixture must deserialize")
}

fn collect_for_test(route: HTTPRoute, adaptive_plugin_key: &str) -> HashSet<AdaptiveTelemetryKey> {
    collect_retained_keys(
        &[Arc::new(route)],
        |plugin_key| plugin_key == adaptive_plugin_key,
        |_| Some(vec!["model-a".to_string(), "model-b".to_string()]),
    )
}

fn route_with_dependencies(route_uid: &str, plugin_name: &str, target_name: &str) -> HTTPRoute {
    let mut route = adaptive_route();
    let rule = &mut route.spec.rules.as_mut().unwrap()[0];
    rule.resolved_terminal_route_uid = Some(route_uid.to_string());
    rule.filters.as_mut().unwrap()[0].extension_ref.as_mut().unwrap().name = plugin_name.to_string();
    rule.backend_refs.as_mut().unwrap()[0].name = target_name.to_string();
    route
}

fn recompute_test_routes(
    index: &mut AdaptiveContributionIndex,
    route_keys: impl IntoIterator<Item = String>,
    adaptive_plugins: &HashSet<String>,
    models: &HashMap<PolicyTargetKey, Vec<String>>,
) {
    let mut plugin_uses_adaptive = |key: &str| adaptive_plugins.contains(key);
    let mut logical_models = |target: &PolicyTargetKey| models.get(target).cloned();
    for route_key in route_keys {
        index.recompute_route(&route_key, &mut plugin_uses_adaptive, &mut logical_models);
    }
}

#[test]
fn current_bucket_is_excluded_until_next_tick_boundary() {
    let state = AdaptiveTelemetryState::new();
    state.tick(100);
    state.record_success_ttft(100, 100);
    state.record_charged_tokens(100, 7);
    state.tick(100);
    assert_eq!(state.snapshot(), AdaptiveSnapshot::default());

    state.tick(101);
    assert_eq!(
        state.snapshot(),
        AdaptiveSnapshot {
            latency_p50_micros: Some(100),
            latency_samples: 1,
            charged_tokens_60s: 7,
        }
    );
}

#[test]
fn p50_uses_successes_only_and_failure_charge_still_counts() {
    let state = AdaptiveTelemetryState::new();
    state.tick(10);
    for value in [1_000, 100, 1_500, 250, 500] {
        state.record_success_ttft(10, value);
    }
    // A failed attempt records only its charged-token accounting.
    state.record_charged_tokens(10, 11);
    state.tick(11);
    assert_eq!(state.snapshot().latency_p50_micros, Some(500));
    assert_eq!(state.snapshot().latency_samples, 5);
    assert_eq!(state.snapshot().charged_tokens_60s, 11);
}

#[test]
fn latency_and_usage_windows_include_only_complete_buckets() {
    let state = AdaptiveTelemetryState::new();
    state.tick(10);
    state.record_charged_tokens(10, 1);
    state.tick(40);
    state.record_success_ttft(40, 100);
    state.record_charged_tokens(40, 4);
    state.tick(69);
    state.record_success_ttft(69, 250);
    state.record_charged_tokens(69, 9);

    state.tick(70);
    let at_70 = state.snapshot();
    assert_eq!(at_70.latency_samples, 2); // seconds 40 and 69
    assert_eq!(at_70.latency_p50_micros, Some(100));
    assert_eq!(at_70.charged_tokens_60s, 14); // seconds 10, 40, and 69

    state.record_success_ttft(70, 500);
    state.record_charged_tokens(70, 10);
    state.tick(71);
    let at_71 = state.snapshot();
    assert_eq!(at_71.latency_samples, 2); // seconds 69 and 70
    assert_eq!(at_71.latency_p50_micros, Some(250));
    assert_eq!(at_71.charged_tokens_60s, 23); // second 10 aged out
}

#[test]
fn token_arithmetic_saturates_and_latency_count_is_bounded() {
    let state = AdaptiveTelemetryState::new();
    state.tick(1);
    state.record_charged_tokens(1, u64::MAX);
    state.record_charged_tokens(1, 1);
    state.tick(2);
    assert_eq!(state.snapshot().charged_tokens_60s, u64::MAX);
}

#[test]
fn equivalent_key_reuses_state_remove_hides_but_old_arc_survives() {
    let store = AdaptiveTelemetryStore::new();
    let key = key("uid-a", "model-a");
    let first = store.get_or_create(key.clone());
    let equivalent = store.get_or_create(key.clone());
    assert!(Arc::ptr_eq(&first, &equivalent));

    first.tick(1);
    first.record_success_ttft(1, 100);
    assert!(store.remove(&key));
    assert!(store.get(&key).is_none());
    first.tick(2);
    assert_eq!(first.snapshot().latency_p50_micros, Some(100));

    let recreated = store.get_or_create(key);
    assert!(!Arc::ptr_eq(&first, &recreated));
    assert_eq!(recreated.snapshot(), AdaptiveSnapshot::default());
}

#[test]
fn count_and_keys_track_telemetry_lifecycle() {
    let store = AdaptiveTelemetryStore::new();
    assert_eq!(store.count(), 0);
    assert!(store.keys().is_empty());

    let key = key("uid-a", "model-a");
    store.reconcile_keys(&HashSet::from([key.clone()]));
    assert_eq!(store.count(), 1);
    assert_eq!(
        store.keys(),
        vec![format!(
            "{}/{}/{}/{}",
            key.terminal_route_uid, key.rule_name, key.target, key.logical_model
        )]
    );

    assert!(store.remove(&key));
    assert_eq!(store.count(), 0);
    assert!(store.keys().is_empty());
}

#[test]
fn concurrent_cold_reconcile_reuses_one_published_arc() {
    let store = Arc::new(AdaptiveTelemetryStore::new());
    let key = key("uid-a", "model-a");
    let barrier = Arc::new(std::sync::Barrier::new(16));
    let threads: Vec<_> = (0..16)
        .map(|_| {
            let store = Arc::clone(&store);
            let key = key.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                store.reconcile_keys(&HashSet::from([key.clone()]));
                store.get(&key).unwrap()
            })
        })
        .collect();
    let states: Vec<_> = threads.into_iter().map(|thread| thread.join().unwrap()).collect();
    let published = store.get(&key).expect("same key must be published");
    assert!(states.iter().all(|state| Arc::ptr_eq(state, &published)));
    assert_eq!(store.count(), 1);
}

#[test]
fn cleanup_retains_only_normalized_keys() {
    let store = AdaptiveTelemetryStore::new();
    let keep = key("uid-a", "model-a");
    let remove = key("uid-a", "model-b");
    let kept_state = store.get_or_create(keep.clone());
    let removed_state = store.get_or_create(remove);
    store.retain_keys(&HashSet::from([keep.clone()]));
    assert_eq!(store.count(), 1);
    assert!(Arc::ptr_eq(&kept_state, &store.get(&keep).unwrap()));
    removed_state.tick(1);
    removed_state.record_charged_tokens(1, 3);
    removed_state.tick(2);
    assert_eq!(removed_state.snapshot().charged_tokens_60s, 3);
}

#[test]
fn reconcile_prebuilds_state_and_request_lookup_never_mutates_snapshot() {
    let store = AdaptiveTelemetryStore::new();
    let key = key("uid-a", "model-a");
    store.reconcile_keys(&HashSet::from([key.clone()]));
    let before = store.states.load_full();
    let state = store.get(&key).expect("cold reconcile must prebuild state");
    for _ in 0..100 {
        assert!(Arc::ptr_eq(&state, &store.get(&key).unwrap()));
    }
    let after = store.states.load_full();

    assert!(Arc::ptr_eq(&before, &after));
}

#[test]
fn target_cleanup_removes_all_models_and_preserves_other_targets() {
    let store = AdaptiveTelemetryStore::new();
    let first = key("route-a", "model-a");
    let second = key("route-b", "model-b");
    let mut other = key("route-c", "model-c");
    other.target.name = "other-provider".to_string();
    let captured = store.get_or_create(first.clone());
    store.get_or_create(second.clone());
    store.get_or_create(other.clone());

    captured.tick(10);
    captured.record_success_ttft(10, 100);
    captured.record_charged_tokens(10, 3);
    captured.tick(11);

    assert_eq!(store.remove_target(&first.target), 2);
    assert!(store.get(&first).is_none());
    assert!(store.get(&second).is_none());
    assert!(store.get(&other).is_some());
    assert_eq!(
        captured.snapshot(),
        AdaptiveSnapshot {
            latency_p50_micros: Some(100),
            latency_samples: 1,
            charged_tokens_60s: 3,
        }
    );

    captured.record_success_ttft(11, 250);
    captured.record_charged_tokens(11, 4);
    captured.tick(12);
    assert_eq!(captured.snapshot().latency_samples, 2);
    assert_eq!(captured.snapshot().charged_tokens_60s, 7);
}

#[test]
fn key_rejects_non_ai_target_and_empty_identity_parts() {
    let target = key("uid", "model").target;
    assert!(AdaptiveTelemetryKey::new("".into(), "rule".into(), target.clone(), "model".into()).is_none());
    assert!(AdaptiveTelemetryKey::new("uid".into(), "".into(), target.clone(), "model".into()).is_none());
    assert!(AdaptiveTelemetryKey::new("uid".into(), "rule".into(), target.clone(), "".into()).is_none());

    let mut target = target;
    target.target_kind = PolicyTargetKind::Service;
    assert!(AdaptiveTelemetryKey::new("uid".into(), "rule".into(), target, "model".into()).is_none());
}

#[test]
fn adaptive_plugin_predicate_requires_enabled_adaptive_ai_proxy() {
    assert!(!request_plugins_use_adaptive_selection(None));
    assert!(!request_plugins_use_adaptive_selection(Some(&[adaptive_entry(
        AiBackendSelection::LowestLatency,
        false,
    )])));
    for selection_type in [
        AiBackendSelection::Weighted,
        AiBackendSelection::Priority,
        AiBackendSelection::CostWeighted,
    ] {
        assert!(!request_plugins_use_adaptive_selection(Some(&[adaptive_entry(
            selection_type,
            true,
        )])));
    }
    for selection_type in [AiBackendSelection::LowestLatency, AiBackendSelection::LowestUsage] {
        assert!(request_plugins_use_adaptive_selection(Some(&[adaptive_entry(
            selection_type,
            true,
        )])));
    }
}

#[test]
fn retained_keys_match_route_plugin_target_and_catalog_models() {
    let retained = collect_for_test(adaptive_route(), "route-ns/adaptive");
    let target = PolicyTargetKey {
        target_kind: PolicyTargetKind::EdgionBackend,
        namespace: "route-ns".to_string(),
        name: "provider".to_string(),
    };
    let expected: HashSet<_> = ["model-a", "model-b"]
        .into_iter()
        .map(|model| {
            AdaptiveTelemetryKey::new(
                "route-uid".to_string(),
                "name:primary".to_string(),
                target.clone(),
                model.to_string(),
            )
            .unwrap()
        })
        .collect();
    assert_eq!(retained, expected);

    let mut cross_namespace = adaptive_route();
    cross_namespace.spec.rules.as_mut().unwrap()[0]
        .backend_refs
        .as_mut()
        .unwrap()[0]
        .namespace = Some("provider-ns".to_string());
    let explicit_target = PolicyTargetKey {
        target_kind: PolicyTargetKind::EdgionBackend,
        namespace: "provider-ns".to_string(),
        name: "provider".to_string(),
    };
    let explicit_retained = collect_retained_keys(
        &[Arc::new(cross_namespace)],
        |plugin_key| plugin_key == "route-ns/adaptive",
        |target| {
            assert_eq!(target, &explicit_target);
            Some(vec!["model-a".to_string()])
        },
    );
    assert_eq!(
        explicit_retained,
        HashSet::from([AdaptiveTelemetryKey::new(
            "route-uid".to_string(),
            "name:primary".to_string(),
            explicit_target,
            "model-a".to_string(),
        )
        .unwrap(),])
    );
}

#[test]
fn retained_keys_fail_closed_for_ineligible_route_shapes() {
    let mut cases = Vec::new();

    let mut non_extension = adaptive_route();
    non_extension.spec.rules.as_mut().unwrap()[0].filters.as_mut().unwrap()[0].filter_type =
        HTTPRouteFilterType::RequestHeaderModifier;
    cases.push(non_extension);

    let mut wrong_group = adaptive_route();
    wrong_group.spec.rules.as_mut().unwrap()[0].filters.as_mut().unwrap()[0]
        .extension_ref
        .as_mut()
        .unwrap()
        .group = "example.io".to_string();
    cases.push(wrong_group);

    let mut wrong_kind = adaptive_route();
    wrong_kind.spec.rules.as_mut().unwrap()[0].filters.as_mut().unwrap()[0]
        .extension_ref
        .as_mut()
        .unwrap()
        .kind = "OtherPlugins".to_string();
    cases.push(wrong_kind);

    let mut unresolved_route = adaptive_route();
    unresolved_route.spec.rules.as_mut().unwrap()[0].resolved_terminal_route_uid = None;
    cases.push(unresolved_route);

    let mut non_ai_backend = adaptive_route();
    let backend = &mut non_ai_backend.spec.rules.as_mut().unwrap()[0]
        .backend_refs
        .as_mut()
        .unwrap()[0];
    backend.group = None;
    backend.kind = Some("Service".to_string());
    cases.push(non_ai_backend);

    for route in cases {
        assert!(collect_for_test(route, "route-ns/adaptive").is_empty());
    }

    assert!(collect_for_test(adaptive_route(), "route-ns/other-plugin").is_empty());
    assert!(collect_retained_keys(
        &[Arc::new(adaptive_route())],
        |plugin_key| plugin_key == "route-ns/adaptive",
        |_| None,
    )
    .is_empty());
}

#[test]
fn contribution_index_self_heals_when_route_arrives_before_dependencies() {
    let route = route_with_dependencies("uid-a", "adaptive", "provider");
    let facts = RouteAdaptiveFacts::from_route(&route);
    let target = facts.targets.iter().next().unwrap().clone();
    let mut index = AdaptiveContributionIndex::default();
    index.attach_route("route-a".to_string(), facts);
    for position in 0..64 {
        index.attach_route(format!("ordinary-{position}"), RouteAdaptiveFacts::default());
    }

    let mut adaptive_plugins = HashSet::new();
    let mut models = HashMap::new();
    let all_routes: Vec<_> = index.routes.keys().cloned().collect();
    recompute_test_routes(&mut index, all_routes, &adaptive_plugins, &models);
    assert!(index.retained_keys().is_empty());
    assert!(index.route_contributions.is_empty());
    assert_eq!(
        index.plugin_routes["route-ns/adaptive"],
        HashSet::from(["route-a".to_string()])
    );
    assert_eq!(index.target_routes[&target], HashSet::from(["route-a".to_string()]));

    adaptive_plugins.insert("route-ns/adaptive".to_string());
    let plugin_routes = index.plugin_routes["route-ns/adaptive"].clone();
    recompute_test_routes(&mut index, plugin_routes, &adaptive_plugins, &models);
    assert!(index.retained_keys().is_empty());
    assert!(index.route_contributions.is_empty());

    models.insert(target, vec!["model-a".to_string(), "model-b".to_string()]);
    recompute_test_routes(&mut index, ["route-a".to_string()], &adaptive_plugins, &models);
    assert_eq!(index.retained_keys().len(), 2);
}

#[test]
fn contribution_index_updates_only_changed_route_and_removes_stale_facts() {
    let route_a = route_with_dependencies("uid-a", "adaptive-a", "provider-a");
    let route_b = route_with_dependencies("uid-b", "adaptive-b", "provider-b");
    let facts_a = RouteAdaptiveFacts::from_route(&route_a);
    let facts_b = RouteAdaptiveFacts::from_route(&route_b);
    let models: HashMap<_, _> = facts_a
        .targets
        .iter()
        .chain(facts_b.targets.iter())
        .cloned()
        .map(|target| (target, vec!["model".to_string()]))
        .collect();
    let adaptive_plugins = HashSet::from(["route-ns/adaptive-a".to_string(), "route-ns/adaptive-b".to_string()]);
    let mut index = AdaptiveContributionIndex::default();
    index.attach_route("route-a".to_string(), facts_a);
    index.attach_route("route-b".to_string(), facts_b);
    recompute_test_routes(
        &mut index,
        ["route-a".to_string(), "route-b".to_string()],
        &adaptive_plugins,
        &models,
    );
    let route_b_keys = index.route_contributions["route-b"].clone();

    index.detach_route("route-a");
    index.attach_route(
        "route-a".to_string(),
        RouteAdaptiveFacts::from_route(&route_with_dependencies("uid-a-next", "adaptive-a", "provider-a")),
    );
    recompute_test_routes(&mut index, ["route-a".to_string()], &adaptive_plugins, &models);
    assert_eq!(index.route_contributions["route-b"], route_b_keys);
    assert!(index
        .retained_keys()
        .iter()
        .any(|key| key.terminal_route_uid == "uid-a-next"));
    assert!(!index
        .retained_keys()
        .iter()
        .any(|key| key.terminal_route_uid == "uid-a"));

    index.detach_route("route-a");
    assert_eq!(index.retained_keys(), route_b_keys);
    assert!(!index.plugin_routes.contains_key("route-ns/adaptive-a"));
}

#[test]
fn duplicate_route_contributions_survive_until_last_source_is_removed() {
    let facts = RouteAdaptiveFacts::from_route(&adaptive_route());
    let target = facts.targets.iter().next().unwrap().clone();
    let adaptive_plugins = HashSet::from(["route-ns/adaptive".to_string()]);
    let models = HashMap::from([(target, vec!["model-a".to_string()])]);
    let mut index = AdaptiveContributionIndex::default();
    index.attach_route("parent-a".to_string(), facts.clone());
    index.attach_route("parent-b".to_string(), facts);
    recompute_test_routes(
        &mut index,
        ["parent-a".to_string(), "parent-b".to_string()],
        &adaptive_plugins,
        &models,
    );
    assert_eq!(index.retained_keys().len(), 1);

    index.detach_route("parent-a");
    assert_eq!(index.retained_keys().len(), 1);
    index.detach_route("parent-b");
    assert!(index.retained_keys().is_empty());
}

#[test]
fn selection_sequence_is_bounded_and_explores_every_twentieth_plan() {
    let store = AdaptiveTelemetryStore::new();
    let sequence: Vec<_> = (0..21).map(|_| store.next_selection_sequence()).collect();
    assert_eq!(sequence, (0..20).chain(std::iter::once(0)).collect::<Vec<_>>());
}

#[tokio::test]
async fn global_ticker_stops_when_auxiliary_shutdown_is_signaled() {
    let store = Arc::new(AdaptiveTelemetryStore::new());
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let task = tokio::spawn(run_adaptive_telemetry_ticker(store, shutdown_rx));
    shutdown_tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .expect("ticker must honor shutdown")
        .expect("ticker task must not panic");
}

#[test]
fn concurrent_recording_is_atomic_and_boundary_rotation_isolated() {
    let state = AdaptiveTelemetryState::new();
    state.tick(5);
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let state = Arc::clone(&state);
            std::thread::spawn(move || {
                for _ in 0..1_000 {
                    state.record_success_ttft(5, 100);
                    state.record_charged_tokens(5, 2);
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    state.tick(6);
    assert_eq!(state.snapshot().latency_samples, 8_000);
    assert_eq!(state.snapshot().latency_p50_micros, Some(100));
    assert_eq!(state.snapshot().charged_tokens_60s, 16_000);

    // A late write to the complete second cannot contaminate the new slot.
    state.record_success_ttft(5, 1);
    state.record_charged_tokens(5, 1);
    state.tick(7);
    assert_eq!(state.snapshot().latency_samples, 8_000);
    assert_eq!(state.snapshot().charged_tokens_60s, 16_000);
}

#[test]
fn invalid_owning_extension_never_queries_adaptive_plugin_or_catalog() {
    for (field, value) in [("name", ""), ("name", "bad/name"), ("group", ""), ("kind", "Secret")] {
        let mut raw = serde_json::to_value(adaptive_route()).unwrap();
        raw["spec"]["rules"][0]["filters"][0]["extensionRef"][field] = serde_json::json!(value);
        let route: HTTPRoute = serde_json::from_value(raw).unwrap();
        let keys = collect_retained_keys(
            &[Arc::new(route)],
            |_| panic!("invalid field must not query plugin"),
            |_| panic!("invalid field must not query catalog"),
        );
        assert!(keys.is_empty(), "{field}");
    }
    let mut route = adaptive_route();
    route.metadata.name = None;
    assert!(collect_retained_keys(
        &[Arc::new(route)],
        |_| panic!("invalid owner queried"),
        |_| panic!("invalid owner queried")
    )
    .is_empty());
}

#[test]
fn bare_plugin_events_find_semantic_adaptive_dependents_on_collision_and_recovery() {
    use edgion_resources::resources::refs::plugin_target::{classify, semantic_key, PluginTarget};
    let mut route = adaptive_route();
    route.metadata.namespace = None;
    let facts = RouteAdaptiveFacts::from_route(&route);
    let target = facts.targets.iter().next().unwrap().clone();
    let mut index = AdaptiveContributionIndex::default();
    index.attach_route("route-a".into(), facts);
    let models = HashMap::from([(target, vec!["model-a".to_string()])]);
    let mut sources = HashMap::new();
    for (raw_key, present) in [
        ("adaptive", true),
        ("default/adaptive", true),
        ("adaptive", false),
        ("default/adaptive", false),
    ] {
        if present {
            sources.insert(raw_key, ());
        } else {
            sources.remove(raw_key);
        }
        let semantic = semantic_key(raw_key).unwrap();
        let routes = index.plugin_routes[&semantic].clone();
        let unique = matches!(
            classify(&semantic, |key| sources.get(key).copied()).unwrap(),
            PluginTarget::Unique { .. }
        );
        let plugins = if unique {
            HashSet::from([semantic])
        } else {
            HashSet::new()
        };
        recompute_test_routes(&mut index, routes, &plugins, &models);
        assert_eq!(index.retained_keys().len(), usize::from(unique));
    }
}
