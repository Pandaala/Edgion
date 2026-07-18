//! Unit tests for `status_metrics.rs` (see `super`).
use super::*;

mod classify_tests {
    use super::*;

    fn c(t: &str, s: &str) -> Condition {
        Condition {
            type_: t.into(),
            status: s.into(),
        }
    }

    #[test]
    fn err_and_warn_coexist_both_preserved() {
        let cs = vec![c("Accepted", "False"), c("Ready", "False")];
        let (err, warn) = classify(&cs).unwrap();
        assert_eq!(err.len(), 1);
        assert_eq!(err[0].type_, "Accepted");
        assert_eq!(warn.len(), 1);
        assert_eq!(warn[0].type_, "Ready");
    }

    #[test]
    fn unknown_type_is_ignored() {
        let cs = vec![c("SomethingElse", "False"), c("Accepted", "True")];
        assert!(classify(&cs).is_none());
    }

    #[test]
    fn plugin_policy_restricted_true_is_err_level() {
        let conds = vec![Condition {
            type_: "PluginPolicyRestricted".to_string(),
            status: "True".to_string(),
        }];
        let (err, warn) = classify(&conds).expect("must classify");
        assert_eq!(err.len(), 1);
        assert_eq!(err[0].type_, "PluginPolicyRestricted");
        assert!(warn.is_empty());
        // False (or absent) is clean:
        let conds = vec![Condition {
            type_: "PluginPolicyRestricted".to_string(),
            status: "False".to_string(),
        }];
        assert!(classify(&conds).is_none());
    }
}

mod scan_conditions_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn gateway_top_level_plus_listeners() {
        let j = json!({
            "status": {
                "conditions": [{"type": "Accepted", "status": "True"}],
                "listeners": [
                    {"name": "http", "conditions": [
                        {"type": "Programmed", "status": "False"},
                        {"type": "Conflicted", "status": "True"}
                    ]}
                ]
            }
        });
        let conds = scan_conditions(&j);
        assert_eq!(conds.len(), 3);
        let types: Vec<&str> = conds.iter().map(|c| c.type_.as_str()).collect();
        assert!(types.contains(&"Accepted"));
        assert!(types.contains(&"Programmed"));
        assert!(types.contains(&"Conflicted"));
    }
}

mod render_tests {
    use super::*;

    #[test]
    fn label_values_escape_backslash_and_quote() {
        let entries = vec![Entry {
            kind: "HTTPRoute",
            namespace: "n\"s".into(),
            name: "back\\slash".into(),
            err_conditions: vec![ConditionRef {
                type_: "Accepted".into(),
                status: "False".into(),
            }],
            warn_conditions: vec![],
        }];
        let out = render_snapshot(&entries);
        assert!(out.contains(r#"namespace="n\"s""#));
        assert!(out.contains(r#"name="back\\slash""#));
    }
}

mod registration_tests {
    use super::*;
    use crate::conf_mgr::sync_runtime::resource_processor::raw_store::{LocalRawStore, RawStore};
    use serde::Serialize;
    use std::sync::Arc;

    #[derive(Clone, Debug, Serialize)]
    struct FakeRoute {
        metadata: FakeMeta,
        status: FakeStatus,
    }
    #[derive(Clone, Debug, Serialize)]
    struct FakeMeta {
        namespace: String,
        name: String,
    }
    #[derive(Clone, Debug, Serialize)]
    struct FakeStatus {
        conditions: Vec<FakeCond>,
    }
    #[derive(Clone, Debug, Serialize)]
    struct FakeCond {
        #[serde(rename = "type")]
        type_: String,
        status: String,
    }

    fn mk(ns: &str, name: &str, type_: &str, status: &str) -> Arc<FakeRoute> {
        Arc::new(FakeRoute {
            metadata: FakeMeta {
                namespace: ns.into(),
                name: name.into(),
            },
            status: FakeStatus {
                conditions: vec![FakeCond {
                    type_: type_.into(),
                    status: status.into(),
                }],
            },
        })
    }

    #[test]
    fn render_kind_emits_entries_only_for_abnormal_resources() {
        let _guard = super::TEST_MUTEX.lock();
        renderers_slot().lock().clear();
        snapshot_slot().store(Arc::new(Vec::new()));
        let store: Arc<dyn RawStore<FakeRoute>> = Arc::new({
            let s = LocalRawStore::<FakeRoute>::new();
            s.upsert("default/bad".into(), mk("default", "bad", "Accepted", "False"));
            s.upsert("default/good".into(), mk("default", "good", "Accepted", "True"));
            s.upsert("default/warn".into(), mk("default", "warn", "Ready", "False"));
            s
        });

        let mut buf: Vec<Entry> = Vec::new();
        render_kind("FakeRoute", store.as_ref(), &mut buf);

        assert_eq!(buf.len(), 2);
        let bad = buf.iter().find(|e| e.name == "bad").unwrap();
        assert_eq!(bad.err_conditions.len(), 1);
        assert_eq!(bad.err_conditions[0].type_, "Accepted");
        assert!(bad.warn_conditions.is_empty());
        assert_eq!(bad.kind, "FakeRoute");
        let warn = buf.iter().find(|e| e.name == "warn").unwrap();
        assert!(warn.err_conditions.is_empty());
        assert_eq!(warn.warn_conditions.len(), 1);
        assert_eq!(warn.warn_conditions[0].type_, "Ready");
    }
}

mod snapshot_loop_tests {
    use super::*;
    use crate::conf_mgr::sync_runtime::resource_processor::raw_store::LocalRawStore;
    use serde::Serialize;
    use std::sync::Arc;

    #[derive(Clone, Serialize)]
    struct Dummy {
        metadata: Meta,
        status: St,
    }
    #[derive(Clone, Serialize)]
    struct Meta {
        namespace: String,
        name: String,
    }
    #[derive(Clone, Serialize)]
    struct St {
        conditions: Vec<Cond>,
    }
    #[derive(Clone, Serialize)]
    struct Cond {
        #[serde(rename = "type")]
        type_: String,
        status: String,
    }

    fn reset() {
        let mut g = renderers_slot().lock();
        g.clear();
        snapshot_slot().store(Arc::new(Vec::new()));
        // Restore default hard cap in case a previous test changed it.
        set_hard_cap(1000);
    }

    fn new_dummy(ns: &str, name: &str, type_: &str, status: &str) -> Arc<Dummy> {
        Arc::new(Dummy {
            metadata: Meta {
                namespace: ns.into(),
                name: name.into(),
            },
            status: St {
                conditions: vec![Cond {
                    type_: type_.into(),
                    status: status.into(),
                }],
            },
        })
    }

    /// Guards against the conf_center-reload duplication bug: a second
    /// `register_status_renderer` call for the same kind must replace the
    /// prior renderer (and drop its Arc to the old RawStore), NOT append.
    /// Otherwise every reload would double the number of entries each kind
    /// contributes per tick.
    #[test]
    fn re_registering_same_kind_replaces_old_renderer() {
        let _guard = super::TEST_MUTEX.lock();
        reset();

        // First registration: one abnormal resource named "old".
        let old_store = Arc::new(LocalRawStore::<Dummy>::new());
        old_store.upsert("a/old".into(), new_dummy("a", "old", "Accepted", "False"));
        register_status_renderer::<Dummy>("Dummy", old_store.clone());

        // Simulated reload: second registration for the SAME kind with a
        // freshly-constructed store exposing a differently-named resource.
        let new_store = Arc::new(LocalRawStore::<Dummy>::new());
        new_store.upsert("a/new".into(), new_dummy("a", "new", "Accepted", "False"));
        register_status_renderer::<Dummy>("Dummy", new_store.clone());

        snapshot_once();
        let snap = snapshot_slot().load();
        // If the bug came back, len would be 2 (one entry per store)
        // AND both "old" and "new" would appear.
        assert_eq!(
            snap.len(),
            1,
            "expected exactly 1 entry (old renderer replaced); got {} entries = {:?}",
            snap.len(),
            snap.iter().map(|e| e.name.clone()).collect::<Vec<_>>()
        );
        assert_eq!(snap[0].name, "new");
    }

    #[test]
    fn snapshot_once_propagates_renderer_panic_but_outer_loop_isolates_it() {
        let _guard = super::TEST_MUTEX.lock();
        reset();
        // Register a panicking renderer.
        renderers_slot().lock().insert(
            "PanickingFake",
            Box::new(|_buf: &mut Vec<Entry>| {
                panic!("synthetic panic");
            }),
        );
        // New contract: panics propagate out of snapshot_once; the outer
        // `spawn_snapshot_loop` task wraps each call in catch_unwind so
        // the background loop survives.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(snapshot_once));
        assert!(result.is_err(), "panic should propagate out of snapshot_once");
    }
}

mod perf_tests {
    use super::*;
    use crate::conf_mgr::sync_runtime::resource_processor::raw_store::LocalRawStore;
    use serde::Serialize;
    use std::sync::Arc;
    use std::time::Instant;

    #[derive(Clone, Serialize)]
    struct Mock {
        metadata: M,
        status: S,
    }
    #[derive(Clone, Serialize)]
    struct M {
        namespace: String,
        name: String,
    }
    #[derive(Clone, Serialize)]
    struct S {
        parents: Vec<P>,
    }
    #[derive(Clone, Serialize)]
    struct P {
        conditions: Vec<C>,
    }
    #[derive(Clone, Serialize)]
    struct C {
        #[serde(rename = "type")]
        type_: String,
        status: String,
    }

    fn reset() {
        renderers_slot().lock().clear();
        snapshot_slot().store(Arc::new(Vec::new()));
        set_hard_cap(1000);
    }

    #[test]
    fn snapshot_1000_routes_under_500ms() {
        let _guard = super::TEST_MUTEX.lock();
        reset();
        let store = Arc::new(LocalRawStore::<Mock>::new());
        for i in 0..1000 {
            let conds = vec![
                C {
                    type_: "Accepted".into(),
                    status: "True".into(),
                },
                C {
                    type_: "ResolvedRefs".into(),
                    status: if i % 5 == 0 { "False".into() } else { "True".into() },
                },
            ];
            let obj = Arc::new(Mock {
                metadata: M {
                    namespace: "ns".into(),
                    name: format!("r{}", i),
                },
                status: S {
                    parents: vec![P { conditions: conds }],
                },
            });
            store.upsert(format!("ns/r{}", i), obj);
        }
        register_status_renderer::<Mock>("Mock", store);

        let t = Instant::now();
        snapshot_once();
        let elapsed = Instant::now().saturating_duration_since(t);
        assert!(
            elapsed.as_millis() < 500,
            "snapshot_once with 1000 routes took {}ms (limit 500ms) — scan_conditions may have regressed",
            elapsed.as_millis()
        );

        let snap = snapshot_slot().load();
        assert_eq!(snap.len(), 200);
    }
}
