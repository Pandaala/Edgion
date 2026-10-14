use super::*;
use crate::conf_mgr::sync_runtime::resource_processor::LocalRawStore;
use edgion_resources::{
    EdgionConfigData, EdgionConfigDataSpec, EdgionPlugins, EdgionPluginsSpec, EdgionStreamPlugins,
    EdgionStreamPluginsSpec, LinkSys, LinkSysSpec,
};

fn reset() {
    sources_slot().lock().clear();
    *sampled_gauges().lock() = std::array::from_fn(|_| None);
    METRIC_RECORDER.with(|slot| {
        *slot.borrow_mut() = Some(
            edgion_common::observe::metrics_api::TestRecorder::new(
                edgion_common::observe::metrics_api::RecorderConfig {
                    service: "test",
                    idle_timeout_secs: 0,
                    histogram_buckets: &[],
                    cardinality_cap: 10000,
                    cardinality_mode: edgion_common::observe::cardinality_guard::CardinalityMode::Enforce,
                },
            )
            .unwrap(),
        )
    });
}

thread_local! {static METRIC_RECORDER:std::cell::RefCell<Option<edgion_common::observe::metrics_api::TestRecorder>>=const {std::cell::RefCell::new(None)};}
fn render_unknown_config_fields_metric() -> String {
    METRIC_RECORDER.with(|slot| {
        let binding = slot.borrow();
        let recorder = binding.as_ref().unwrap();
        recorder.with(sample_unknown_config_fields_metric);
        let rendered = recorder.render();
        let mut output = String::new();
        for line in rendered.lines().filter(|line| line.contains(METRIC_NAME)) {
            output.push_str(&line.replace("service=\"test\",", ""));
            output.push('\n');
        }
        output
    })
}

fn plugins_with_findings(ns: &str, name: &str, paths: Vec<String>) -> Arc<EdgionPlugins> {
    let mut ep = EdgionPlugins::new(name, EdgionPluginsSpec::default());
    ep.metadata.namespace = Some(ns.to_string());
    ep.spec.unknown_fields = paths;
    Arc::new(ep)
}

fn stream_plugins_with_findings(ns: &str, name: &str, paths: Vec<String>) -> Arc<EdgionStreamPlugins> {
    let mut esp = EdgionStreamPlugins::new(
        name,
        EdgionStreamPluginsSpec {
            current_status: None,
            plugins: None,
            tls_route_plugins: None,
            unknown_fields: paths,
        },
    );
    esp.metadata.namespace = Some(ns.to_string());
    Arc::new(esp)
}

fn config_data_with_findings(ns: &str, name: &str, paths: Vec<String>) -> Arc<EdgionConfigData> {
    let spec: EdgionConfigDataSpec = serde_json::from_value(serde_json::json!({
        "data": {"type": "Misc", "config": {}}
    }))
    .expect("minimal ConfigData spec");
    let mut data = EdgionConfigData::new(name, spec);
    data.metadata.namespace = Some(ns.to_string());
    data.spec.unknown_fields = paths;
    Arc::new(data)
}

fn link_sys_with_findings(ns: &str, name: &str, paths: Vec<String>) -> Arc<LinkSys> {
    let spec: LinkSysSpec = serde_json::from_value(serde_json::json!({
        "type": "redis", "config": {"endpoints": []}
    }))
    .expect("minimal LinkSys spec");
    let mut link_sys = LinkSys::new(name, spec);
    link_sys.metadata.namespace = Some(ns.to_string());
    link_sys.spec.unknown_fields = paths;
    Arc::new(link_sys)
}

#[test]
fn empty_registry_yields_no_findings() {
    let _guard = TEST_MUTEX.lock();
    reset();
    let (total, truncated, items) = collect_unknown_fields(100);
    assert_eq!(total, 0);
    assert!(!truncated);
    assert!(items.is_empty());
}

#[test]
fn reports_findings_from_all_supported_kinds() {
    let _guard = TEST_MUTEX.lock();
    reset();

    let plugins_store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
    plugins_store.upsert(
        "default/auth-chain".into(),
        plugins_with_findings("default", "auth-chain", vec!["requestPlugins[0].config.denys".into()]),
    );
    register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", plugins_store);

    let stream_store = Arc::new(LocalRawStore::<EdgionStreamPlugins>::new());
    stream_store.upsert(
        "default/geo".into(),
        stream_plugins_with_findings("default", "geo", vec!["plugins[0].config.typo".into()]),
    );
    register_unknown_fields_source::<EdgionStreamPlugins>("EdgionStreamPlugins", stream_store);

    let config_data_store = Arc::new(LocalRawStore::<EdgionConfigData>::new());
    config_data_store.upsert(
        "default/cidrs".into(),
        config_data_with_findings("default", "cidrs", vec!["data.config.items[0].cidrz".into()]),
    );
    register_unknown_fields_source::<EdgionConfigData>("EdgionConfigData", config_data_store);

    let link_sys_store = Arc::new(LocalRawStore::<LinkSys>::new());
    link_sys_store.upsert(
        "default/redis".into(),
        link_sys_with_findings("default", "redis", vec!["config.timeout.conect".into()]),
    );
    register_unknown_fields_source::<LinkSys>("LinkSys", link_sys_store);

    let (total, truncated, items) = collect_unknown_fields(100);
    assert_eq!(total, 4);
    assert!(!truncated);
    assert_eq!(items.len(), 4);
    assert_eq!(items[0].kind, "EdgionConfigData");
    assert_eq!(items[0].paths, vec!["data.config.items[0].cidrz".to_string()]);
    assert_eq!(items[1].kind, "EdgionPlugins");
    assert_eq!(items[2].kind, "EdgionStreamPlugins");
    assert_eq!(items[3].kind, "LinkSys");
}

#[test]
fn clean_resources_are_not_reported() {
    let _guard = TEST_MUTEX.lock();
    reset();

    let store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
    store.upsert(
        "default/clean".into(),
        plugins_with_findings("default", "clean", Vec::new()),
    );
    register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", store);

    let (total, truncated, items) = collect_unknown_fields(100);
    assert_eq!(total, 0);
    assert!(!truncated);
    assert!(items.is_empty());
}

#[test]
fn other_kinds_are_ignored_without_registering_a_source() {
    let _guard = TEST_MUTEX.lock();
    reset();

    // A kind with no `unknown_fields` field: register_unknown_fields_source
    // must be a cheap no-op, never occupying a registry slot.
    let store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
    register_unknown_fields_source::<EdgionPlugins>("HTTPRoute", store);
    assert!(sources_slot().lock().is_empty());
}

#[test]
fn cap_truncates_and_reports_honestly() {
    let _guard = TEST_MUTEX.lock();
    reset();

    let store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
    for i in 0..5 {
        store.upsert(
            format!("default/p{i}"),
            plugins_with_findings("default", &format!("p{i}"), vec!["typo".into()]),
        );
    }
    register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", store);

    let (total, truncated, items) = collect_unknown_fields(2);
    assert_eq!(total, 5, "total must report the pre-cap count");
    assert!(truncated);
    assert_eq!(items.len(), 2, "items must honor the cap");
}

#[test]
fn bounded_retention_keeps_deterministic_first_entries_and_caps_paths() {
    let _guard = TEST_MUTEX.lock();
    reset();

    let store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
    for name in ["z-last", "a-first", "m-middle"] {
        let paths = (0..(MAX_PATHS_PER_RESOURCE + 3))
            .map(|i| format!("very.long.path.{i}.{}", "x".repeat(1024)))
            .collect();
        store.upsert(format!("default/{name}"), plugins_with_findings("default", name, paths));
    }
    register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", store);

    let (total, truncated, items) = collect_unknown_fields(2);
    assert_eq!(total, 3);
    assert!(truncated);
    assert_eq!(
        items.iter().map(|item| item.name.as_str()).collect::<Vec<_>>(),
        ["a-first", "m-middle"]
    );
    assert_eq!(items[0].paths.len(), MAX_PATHS_PER_RESOURCE + 1);
    assert_eq!(items[0].paths[MAX_PATHS_PER_RESOURCE], "(+3 more)");
    assert!(items[0].paths[..MAX_PATHS_PER_RESOURCE]
        .iter()
        .all(|path| path.len() <= MAX_UNKNOWN_FIELD_PATH_BYTES && path.contains("bytes)")));
}

#[test]
fn diagnostics_bound_oversized_identity_components() {
    let _guard = TEST_MUTEX.lock();
    reset();

    let store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
    store.upsert(
        "oversized".into(),
        plugins_with_findings(&"n".repeat(2048), &"r".repeat(2048), vec!["config.typo".to_string()]),
    );
    register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", store);

    let (_, _, items) = collect_unknown_fields(1);
    assert_eq!(items.len(), 1);
    assert!(items[0].namespace.len() <= MAX_UNKNOWN_FIELD_PATH_BYTES);
    assert!(items[0].name.len() <= MAX_UNKNOWN_FIELD_PATH_BYTES);
    assert!(items[0].namespace.contains("bytes; sha256:"));
    assert!(items[0].name.contains("bytes; sha256:"));
}

#[test]
fn oversized_identity_ties_are_stably_disambiguated_before_bounded_retention() {
    let common_prefix = "r".repeat(MAX_UNKNOWN_FIELD_PATH_BYTES + 128);
    let first = bounded_resource_identity(&format!("{common_prefix}a"));
    let second = bounded_resource_identity(&format!("{common_prefix}b"));
    assert_ne!(first, second, "the digest must preserve a stable distinction");
    assert!(first.len() <= MAX_UNKNOWN_FIELD_PATH_BYTES);
    assert!(second.len() <= MAX_UNKNOWN_FIELD_PATH_BYTES);

    let entry = |name: String| UnknownFieldsEntry {
        kind: "EdgionPlugins",
        namespace: "default".to_string(),
        name,
        paths: vec!["config.typo".to_string()],
    };
    let expected = first.clone().min(second.clone());

    let mut forward = BoundedCollector::new(1);
    forward.push(entry(first.clone()));
    forward.push(entry(second.clone()));

    let mut reverse = BoundedCollector::new(1);
    reverse.push(entry(second));
    reverse.push(entry(first));

    assert_eq!(forward.items[0].name, expected);
    assert_eq!(reverse.items[0].name, expected);
}

/// Guards against the conf_center-reload duplication bug (see the module
/// doc comment): a second `register_unknown_fields_source` call for the
/// same kind must replace the prior source, not accumulate a duplicate.
#[test]
fn re_registering_same_kind_replaces_old_source() {
    let _guard = TEST_MUTEX.lock();
    reset();

    let old_store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
    old_store.upsert(
        "default/old".into(),
        plugins_with_findings("default", "old", vec!["old.path".into()]),
    );
    register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", old_store);

    // Simulated reload: second registration for the SAME kind with a
    // freshly-constructed store exposing a differently-named resource.
    let new_store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
    new_store.upsert(
        "default/new".into(),
        plugins_with_findings("default", "new", vec!["new.path".into()]),
    );
    register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", new_store);

    let (total, truncated, items) = collect_unknown_fields(100);
    assert_eq!(total, 1, "expected exactly 1 entry (old source replaced)");
    assert!(!truncated);
    assert_eq!(items[0].name, "new");
}

mod metric_rendering {
    use super::*;

    #[test]
    fn healthy_fleet_emits_no_series_and_no_block() {
        let _guard = TEST_MUTEX.lock();
        reset();

        let store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
        store.upsert(
            "default/clean".into(),
            plugins_with_findings("default", "clean", Vec::new()),
        );
        register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", store);

        let block = render_unknown_config_fields_metric();
        assert!(
            block.is_empty(),
            "a healthy fleet must render nothing, not a bare HELP/TYPE: {block:?}"
        );
    }

    #[test]
    fn one_finding_renders_one_series_per_kind_with_resource_count_as_value() {
        let _guard = TEST_MUTEX.lock();
        reset();

        let store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
        store.upsert(
            "default/demo-bad".into(),
            plugins_with_findings("default", "demo-bad", vec!["requestPlugins[0].config.denys".into()]),
        );
        register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", store);

        let block = render_unknown_config_fields_metric();
        let expected = concat!(
                "# HELP edgion_controller_unknown_config_fields ",
                "Resources whose submitted document contains a key the EdgionPlugins, EdgionStreamPlugins, EdgionConfigData, or LinkSys schema does not recognize, aggregated by resource kind. Detailed findings are available from GET /api/v1/diagnostics/unknown-fields. Advisory only; does not affect Accepted or block sync.\n",
                "# TYPE edgion_controller_unknown_config_fields gauge\n",
                "edgion_controller_unknown_config_fields{kind=\"EdgionPlugins\"} 1\n",
            );
        assert_eq!(block, expected);
    }

    #[test]
    fn two_affected_resources_of_the_same_kind_aggregate_into_one_series() {
        let _guard = TEST_MUTEX.lock();
        reset();

        let store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
        store.upsert(
            "default/demo-bad".into(),
            plugins_with_findings("default", "demo-bad", vec!["requestPlugins[0].config.denys".into()]),
        );
        store.upsert(
            "default/other-bad".into(),
            plugins_with_findings("default", "other-bad", vec!["plugins[0].config.typo".into()]),
        );
        register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", store);

        let block = render_unknown_config_fields_metric();
        let samples: Vec<&str> = block.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(samples.len(), 1, "one kind must still be exactly one series: {block}");
        let line = samples[0];
        assert!(line.starts_with("edgion_controller_unknown_config_fields{kind=\"EdgionPlugins\""));
        assert!(
            line.ends_with("} 2\n") || line.ends_with("} 2"),
            "value must be the affected-resource count: {line}"
        );

        assert!(
            !line.contains("warn="),
            "metric must not expose finding data as a label"
        );
    }

    #[test]
    fn four_kinds_each_get_their_own_series() {
        let _guard = TEST_MUTEX.lock();
        reset();

        let plugins_store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
        plugins_store.upsert(
            "default/auth-chain".into(),
            plugins_with_findings("default", "auth-chain", vec!["requestPlugins[0].config.denys".into()]),
        );
        register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", plugins_store);

        let stream_store = Arc::new(LocalRawStore::<EdgionStreamPlugins>::new());
        stream_store.upsert(
            "default/geo".into(),
            stream_plugins_with_findings("default", "geo", vec!["plugins[0].config.typo".into()]),
        );
        register_unknown_fields_source::<EdgionStreamPlugins>("EdgionStreamPlugins", stream_store);

        let config_data_store = Arc::new(LocalRawStore::<EdgionConfigData>::new());
        config_data_store.upsert(
            "default/cidrs".into(),
            config_data_with_findings("default", "cidrs", vec!["data.config.items[0].cidrz".into()]),
        );
        register_unknown_fields_source::<EdgionConfigData>("EdgionConfigData", config_data_store);

        let link_sys_store = Arc::new(LocalRawStore::<LinkSys>::new());
        link_sys_store.upsert(
            "default/redis".into(),
            link_sys_with_findings("default", "redis", vec!["config.timeout.conect".into()]),
        );
        register_unknown_fields_source::<LinkSys>("LinkSys", link_sys_store);

        let block = render_unknown_config_fields_metric();
        let mut samples: Vec<&str> = block.lines().filter(|l| !l.starts_with('#')).collect();
        samples.sort_unstable();
        assert_eq!(samples.len(), 4, "at most one series per kind: {block}");
        assert!(samples[0].starts_with("edgion_controller_unknown_config_fields{kind=\"EdgionConfigData\""));
        assert!(samples[1].starts_with("edgion_controller_unknown_config_fields{kind=\"EdgionPlugins\""));
        assert!(samples[2].starts_with("edgion_controller_unknown_config_fields{kind=\"EdgionStreamPlugins\""));
        assert!(samples[3].starts_with("edgion_controller_unknown_config_fields{kind=\"LinkSys\""));
    }

    #[test]
    fn path_content_never_enters_metric_labels() {
        let _guard = TEST_MUTEX.lock();
        reset();

        let store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
        store.upsert(
            "default/weird".into(),
            plugins_with_findings("default", "weird", vec![r#"config."quoted\field""#.into()]),
        );
        register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", store);

        let block = render_unknown_config_fields_metric();
        assert!(!block.contains("quoted"));
        assert!(!block.contains("warn="));
    }

    #[test]
    fn per_resource_paths_truncate_past_the_cap_with_visible_marker() {
        let _guard = TEST_MUTEX.lock();
        reset();

        let many: Vec<String> = (0..(MAX_PATHS_PER_RESOURCE + 7)).map(|i| format!("path{i}")).collect();
        let store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
        store.upsert(
            "default/many".into(),
            plugins_with_findings("default", "many", many.clone()),
        );
        register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", store);

        let (_, _, items) = collect_unknown_fields(1);
        let paths = &items[0].paths;
        assert_eq!(
            paths.len(),
            MAX_PATHS_PER_RESOURCE + 1,
            "shown paths + one marker entry"
        );
        assert_eq!(paths[MAX_PATHS_PER_RESOURCE], "(+7 more)");
        for (i, p) in many.iter().take(MAX_PATHS_PER_RESOURCE).enumerate() {
            assert_eq!(&paths[i], p);
        }
    }

    #[test]
    fn metric_reports_true_count_without_retaining_diagnostic_entries() {
        let _guard = TEST_MUTEX.lock();
        reset();

        let resource_count = 200;
        let store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
        for i in 0..resource_count {
            store.upsert(
                format!("default/p{i:04}"),
                plugins_with_findings("default", &format!("p{i:04}"), vec!["config.typo".into()]),
            );
        }
        register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", store);

        let block = render_unknown_config_fields_metric();
        let line = block.lines().last().unwrap();
        assert!(
            line.ends_with(&format!("}} {resource_count}")) || line.ends_with(&format!("}} {resource_count}\n")),
            "value must be the untruncated resource count: {line}"
        );
        assert!(!line.contains("warn="));
    }

    /// Pin: rendering an unchanged fleet twice must produce byte-identical
    /// output. A non-deterministic label value would mint a brand-new
    /// series identity on every scrape (far worse than the per-resource
    /// cardinality this per-kind design was built to avoid).
    #[test]
    fn rendering_is_deterministic_across_repeated_calls() {
        let _guard = TEST_MUTEX.lock();
        reset();

        let plugins_store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
        for i in 0..30 {
            plugins_store.upsert(
                format!("default/p{i:03}"),
                plugins_with_findings("default", &format!("p{i:03}"), vec![format!("config.typo{i}")]),
            );
        }
        register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", plugins_store);

        let stream_store = Arc::new(LocalRawStore::<EdgionStreamPlugins>::new());
        stream_store.upsert(
            "default/geo".into(),
            stream_plugins_with_findings("default", "geo", vec!["plugins[0].config.typo".into()]),
        );
        register_unknown_fields_source::<EdgionStreamPlugins>("EdgionStreamPlugins", stream_store);

        let first = render_unknown_config_fields_metric();
        let second = render_unknown_config_fields_metric();
        assert_eq!(
            first, second,
            "repeated rendering of an unchanged fleet must be byte-identical"
        );
    }

    /// Pin: this metric must remain independent of
    /// `status_metrics::classify()` / the abnormal-gauge pin test —
    /// a resource carrying only `UnknownConfigFields=True` on its
    /// status conditions produces no `resource_abnormal` series (see
    /// `status_metrics_test.rs::unknown_config_fields_true_produces_no_series`),
    /// while this metric is driven purely by `spec.unknown_fields`
    /// findings and knows nothing about status conditions at all.
    #[test]
    fn metric_is_independent_of_status_conditions() {
        let _guard = TEST_MUTEX.lock();
        reset();
        // No source registered at all: no status conditions exist
        // anywhere in this test, and the metric must still render
        // correctly from spec-level findings alone.
        let store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
        store.upsert(
            "default/spec-only".into(),
            plugins_with_findings("default", "spec-only", vec!["config.typo".into()]),
        );
        register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", store);

        let block = render_unknown_config_fields_metric();
        assert!(block.contains("edgion_controller_unknown_config_fields{kind=\"EdgionPlugins\""));
    }
}

#[test]
fn initialized_fixed_unknown_field_gauge_recovers_to_zero() {
    let _guard = TEST_MUTEX.lock();
    reset();
    let store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
    store.upsert(
        "default/metric-recovery".into(),
        plugins_with_findings("default", "metric-recovery", vec!["config.typo".into()]),
    );
    register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", store.clone());
    assert!(render_unknown_config_fields_metric().contains("{kind=\"EdgionPlugins\"} 1"));
    store.upsert(
        "default/metric-recovery".into(),
        plugins_with_findings("default", "metric-recovery", vec![]),
    );
    assert!(render_unknown_config_fields_metric().contains("{kind=\"EdgionPlugins\"} 0"));
    assert!(!render_unknown_config_fields_metric().contains("metric-recovery"));
}
