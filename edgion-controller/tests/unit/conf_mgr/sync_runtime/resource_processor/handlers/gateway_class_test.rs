use super::*;
use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::prelude_resources::EdgionGatewayConfig;
use edgion_resources::resources::edgion_gateway_config::EdgionGatewayConfigSpec;
use edgion_resources::resources::gateway_class::{GatewayClassSpec, ParametersReference};
use kube::api::ObjectMeta;
use std::sync::Arc;

struct NoopEgcHandler;
impl ProcessorHandler<EdgionGatewayConfig> for NoopEgcHandler {}

/// Build a context. When `egc_names` is `Some`, an EdgionGatewayConfig processor is
/// registered and seeded with those (cluster-scoped) names; when `None`, no EGC processor
/// is registered (simulating the `no_watch_kinds` opt-out → existence unknown).
fn make_ctx(egc_names: Option<&[&str]>) -> HandlerContext {
    let runtime = Arc::new(ControllerRuntime::default());

    if let Some(names) = egc_names {
        let store = Arc::new(LocalRawStore::<EdgionGatewayConfig>::new());
        for name in names {
            let egc = EdgionGatewayConfig::new(name, EdgionGatewayConfigSpec::default());
            store.upsert(name.to_string(), Arc::new(egc));
        }
        // ResourceProcessor::new spawns a Tokio task; get_raw_any itself is sync-safe.
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let processor = rt.block_on(async {
            Arc::new(ResourceProcessor::<EdgionGatewayConfig>::new(
                "EdgionGatewayConfig",
                100,
                Arc::new(NoopEgcHandler),
                runtime.clone(),
            ))
        });
        processor.set_raw_store(store);
        runtime.processor_registry.register(processor);
    }

    HandlerContext::new(None, None, TriggerChain::default(), 3, 20, runtime)
}

fn make_gateway_class(params_ref: Option<ParametersReference>) -> GatewayClass {
    GatewayClass {
        metadata: ObjectMeta {
            name: Some("edgion".to_string()),
            generation: Some(1),
            ..Default::default()
        },
        spec: GatewayClassSpec {
            current_status: None,
            controller_name: DEFAULT_CONTROLLER_NAME.to_string(),
            description: None,
            parameters_ref: params_ref,
        },
        status: None,
    }
}

fn egc_ref(name: &str) -> ParametersReference {
    ParametersReference {
        group: "edgion.io".to_string(),
        kind: "EdgionGatewayConfig".to_string(),
        name: name.to_string(),
        namespace: None,
    }
}

// Helper that reads a condition's (status, reason) by type from the GatewayClass status.
fn cond(gc: &GatewayClass, type_: &str) -> Option<(String, String)> {
    gc.status
        .as_ref()?
        .conditions
        .iter()
        .find(|c| c.type_ == type_)
        .map(|c| (c.status.clone(), c.reason.clone()))
}

#[test]
fn params_ref_absent_omits_resolved_refs_and_accepts() {
    let handler = GatewayClassHandler::default();
    let ctx = make_ctx(None);
    let mut gc = make_gateway_class(None);

    handler.update_status(&mut gc, &ctx, &[]);

    assert_eq!(cond(&gc, "ResolvedRefs"), None, "no parametersRef → no ResolvedRefs");
    assert_eq!(
        cond(&gc, "Accepted"),
        Some(("True".to_string(), "Accepted".to_string()))
    );
}

#[test]
fn params_ref_bad_group_kind_is_invalid_parameters() {
    let handler = GatewayClassHandler::default();
    let ctx = make_ctx(Some(&[]));
    let bad = ParametersReference {
        group: "wrong.io".to_string(),
        kind: "EdgionGatewayConfig".to_string(),
        name: "whatever".to_string(),
        namespace: None,
    };
    let mut gc = make_gateway_class(Some(bad));

    handler.update_status(&mut gc, &ctx, &[]);

    assert_eq!(
        cond(&gc, "ResolvedRefs"),
        Some(("False".to_string(), "InvalidParameters".to_string()))
    );
    assert_eq!(
        cond(&gc, "Accepted"),
        Some(("False".to_string(), "InvalidParameters".to_string()))
    );
}

#[test]
fn params_ref_missing_target_is_invalid_parameters() {
    let handler = GatewayClassHandler::default();
    let ctx = make_ctx(Some(&[])); // EGC processor present but empty
    let mut gc = make_gateway_class(Some(egc_ref("does-not-exist")));

    handler.update_status(&mut gc, &ctx, &[]);

    assert_eq!(
        cond(&gc, "ResolvedRefs"),
        Some(("False".to_string(), "InvalidParameters".to_string()))
    );
    assert_eq!(
        cond(&gc, "Accepted"),
        Some(("False".to_string(), "InvalidParameters".to_string()))
    );
}

#[test]
fn params_ref_existing_target_resolves() {
    let handler = GatewayClassHandler::default();
    let ctx = make_ctx(Some(&["my-egc"]));
    let mut gc = make_gateway_class(Some(egc_ref("my-egc")));

    handler.update_status(&mut gc, &ctx, &[]);

    assert_eq!(
        cond(&gc, "ResolvedRefs"),
        Some(("True".to_string(), "ResolvedRefs".to_string()))
    );
    assert_eq!(
        cond(&gc, "Accepted"),
        Some(("True".to_string(), "Accepted".to_string()))
    );
}

#[test]
fn egc_processor_absent_treats_existence_as_unknown() {
    let handler = GatewayClassHandler::default();
    let ctx = make_ctx(None); // no EGC processor registered (no_watch_kinds opt-out)
    let mut gc = make_gateway_class(Some(egc_ref("my-egc")));

    handler.update_status(&mut gc, &ctx, &[]);

    // Unknown existence: do not assert ResolvedRefs, do not force Accepted=False.
    assert_eq!(cond(&gc, "ResolvedRefs"), None);
    assert_eq!(
        cond(&gc, "Accepted"),
        Some(("True".to_string(), "Accepted".to_string()))
    );
}

#[test]
fn removing_params_ref_prunes_stale_resolved_refs() {
    let handler = GatewayClassHandler::default();

    // First pass: missing target writes ResolvedRefs=False.
    let ctx = make_ctx(Some(&[]));
    let mut gc = make_gateway_class(Some(egc_ref("gone")));
    handler.update_status(&mut gc, &ctx, &[]);
    assert!(cond(&gc, "ResolvedRefs").is_some());

    // Second pass on the same object after parametersRef is removed → prune.
    gc.spec.parameters_ref = None;
    handler.update_status(&mut gc, &ctx, &[]);
    assert_eq!(cond(&gc, "ResolvedRefs"), None, "stale ResolvedRefs must be pruned");
    assert_eq!(
        cond(&gc, "Accepted"),
        Some(("True".to_string(), "Accepted".to_string()))
    );
}

#[test]
fn invalid_parameters_do_not_resolve_decoys_or_skip_unwatched_validation() {
    let handler = GatewayClassHandler::default();
    for watched in [false, true] {
        let ctx = make_ctx(watched.then_some(&["config", "bad/name", ""][..]));
        for (name, namespace) in [
            ("config", Some("")),
            ("config", Some("default")),
            ("bad/name", None),
            ("", None),
        ] {
            let mut reference = egc_ref(name);
            reference.namespace = namespace.map(str::to_owned);
            let mut class = make_gateway_class(Some(reference));
            handler.update_status(&mut class, &ctx, &[]);
            assert_eq!(
                cond(&class, "ResolvedRefs"),
                Some(("False".into(), "InvalidParameters".into()))
            );
            assert_eq!(
                cond(&class, "Accepted"),
                Some(("False".into(), "InvalidParameters".into()))
            );
            class.spec.parameters_ref = Some(egc_ref("config"));
            handler.update_status(&mut class, &ctx, &[]);
            assert_eq!(cond(&class, "Accepted"), Some(("True".into(), "Accepted".into())));
            assert_eq!(
                cond(&class, "ResolvedRefs"),
                watched.then(|| ("True".into(), "ResolvedRefs".into()))
            );
        }
    }
}
