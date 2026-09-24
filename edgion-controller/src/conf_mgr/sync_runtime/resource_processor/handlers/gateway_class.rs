//! GatewayClass Handler
//!
//! Handles GatewayClass resources.

#[cfg(test)]
use crate::conf_mgr::conf_center::common::DEFAULT_CONTROLLER_NAME;
use crate::conf_mgr::sync_runtime::resource_processor::status_ownership::gateway_has_class_owner;
use crate::conf_mgr::sync_runtime::resource_processor::status_utils::condition_unknown;
use crate::conf_mgr::sync_runtime::resource_processor::{
    condition_false, condition_reasons, condition_true, condition_types, update_condition, HandlerContext,
    ProcessResult, ProcessorHandler,
};
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::prelude_resources::{EdgionGatewayConfig, EdgionPlugins, Gateway, GatewayClass};
use edgion_resources::resources::gateway_class::{GatewayClassStatus, SupportedFeature, SUPPORTED_FEATURES};

/// Outcome of resolving `GatewayClass.spec.parametersRef`.
enum ParamsRefState {
    /// No parametersRef, or the EdgionGatewayConfig kind is not watched — nothing to assert.
    Skip,
    /// parametersRef present, group/kind valid, and the target EdgionGatewayConfig exists.
    Resolved,
    /// The target exists, but a deployment-local early plugin attachment is invalid.
    ResolvedInvalid(String),
    /// parametersRef present but unusable: bad group/kind, or target not found.
    Invalid(String),
}

/// Resolve `parametersRef` synchronously against the processor registry.
///
/// EdgionGatewayConfig is cluster-scoped, so its cache key is the bare `name`. The lookup
/// reads the RawStore, which the watch/file layer populates upstream of the processor
/// pipeline, so a GatewayClass requeued by an EGC change always observes the current state.
fn resolve_params_ref(gc: &GatewayClass, ctx: &HandlerContext) -> ParamsRefState {
    let Some(params_ref) = gc.spec.parameters_ref.as_ref() else {
        return ParamsRefState::Skip;
    };
    if params_ref.group != "edgion.io" || params_ref.kind != "EdgionGatewayConfig" {
        return ParamsRefState::Invalid(format!(
            "parametersRef must reference edgion.io/EdgionGatewayConfig, got \"{}/{}\"",
            params_ref.group, params_ref.kind
        ));
    }
    match ctx.runtime().processor_registry.get("EdgionGatewayConfig") {
        // EGC kind not watched (no_watch_kinds opt-out): existence unknown, do not assert.
        None => ParamsRefState::Skip,
        Some(processor) => {
            let Some(egc) = processor
                .get_raw_any(&params_ref.name)
                .and_then(|raw| raw.downcast::<EdgionGatewayConfig>().ok())
            else {
                return ParamsRefState::Invalid(format!(
                    "referenced EdgionGatewayConfig \"{}\" not found",
                    params_ref.name
                ));
            };
            // Unlike the shared cluster-scoped EGC, this class is owned by one
            // deployment and may report that deployment's local plugin validation.
            let mut errors = Vec::new();
            if let Some(plugins_processor) = ctx.runtime().processor_registry.get("EdgionPlugins") {
                for reference in egc.spec.global_plugins_ref.as_deref().unwrap_or_default() {
                    let namespace = reference.namespace.as_deref().unwrap_or("default");
                    let key = format!("{namespace}/{}", reference.name);
                    if let Some(plugins) = plugins_processor
                        .get_raw_any(&key)
                        .and_then(|raw| raw.downcast::<EdgionPlugins>().ok())
                    {
                        errors.extend(
                            plugins
                                .validate_request_id_early_attachment()
                                .into_iter()
                                .map(|error| format!("globalPluginsRef {key}: {error}")),
                        );
                    }
                }
            }
            if errors.is_empty() {
                ParamsRefState::Resolved
            } else {
                ParamsRefState::ResolvedInvalid(errors.join("; "))
            }
        }
    }
}

/// Deployment ownership of every class with the configured controllerName.
pub struct GatewayClassOwnership {
    controller_name: String,
}

impl GatewayClassOwnership {
    pub(super) fn owns_gateway(&self, gateway: &Gateway, runtime: &ControllerRuntime) -> bool {
        gateway_has_class_owner(gateway, &self.controller_name, runtime)
    }

    pub fn new(controller_name: String) -> Self {
        Self { controller_name }
    }

    pub(super) fn matches(&self, gc: &GatewayClass) -> bool {
        gc.spec.controller_name == self.controller_name
    }
}

/// GatewayClass handler
pub struct GatewayClassHandler {
    ownership: GatewayClassOwnership,
}

impl GatewayClassHandler {
    pub fn new(ownership: GatewayClassOwnership) -> Self {
        Self { ownership }
    }
}

#[cfg(test)]
impl Default for GatewayClassHandler {
    fn default() -> Self {
        Self::new(GatewayClassOwnership::new(DEFAULT_CONTROLLER_NAME.to_string()))
    }
}

/// Ownership can make a Gateway absent from ServerCache, so recovery must
/// inspect raw keys. Publish/unpublish first: the consumer checks that the raw
/// class incarnation has actually been published by this deployment.
async fn requeue_class_gateways(gc: &GatewayClass, ctx: &HandlerContext) {
    let Some(name) = gc.metadata.name.as_deref() else {
        return;
    };
    let Some(processor) = ctx.runtime().processor_registry.get("Gateway") else {
        return;
    };
    for key in processor.raw_keys() {
        if processor
            .get_raw_any(&key)
            .and_then(|raw| raw.downcast::<Gateway>().ok())
            .is_some_and(|gateway| gateway.spec.gateway_class_name == name)
        {
            ctx.requeue("Gateway", key).await;
        }
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<GatewayClass> for GatewayClassHandler {
    fn filter(&self, gc: &GatewayClass, _ctx: &HandlerContext) -> bool {
        gc.spec.controller_name == self.ownership.controller_name
    }

    fn should_publish(&self, gc: &GatewayClass, _ctx: &HandlerContext) -> bool {
        self.ownership.matches(gc)
    }

    async fn after_publish(&self, gc: &GatewayClass, ctx: &HandlerContext) {
        requeue_class_gateways(gc, ctx).await;
    }

    async fn after_unpublish(&self, gc: &GatewayClass, ctx: &HandlerContext) {
        requeue_class_gateways(gc, ctx).await;
    }

    fn validate(&self, _gc: &GatewayClass, _ctx: &HandlerContext) -> Vec<String> {
        // parametersRef resolution (group/kind validity + target existence) is handled in
        // update_status() so that all reference-resolution logic lives in one place and
        // drives both the Accepted and ResolvedRefs conditions consistently.
        Vec::new()
    }

    async fn parse(&self, gc: GatewayClass, _ctx: &HandlerContext) -> ProcessResult<GatewayClass> {
        ProcessResult::Continue(gc)
    }

    fn update_status(&self, gc: &mut GatewayClass, ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = gc.metadata.generation;
        let ref_state = resolve_params_ref(gc, ctx);
        let status = gc.status.get_or_insert_with(GatewayClassStatus::default);

        // ResolvedRefs condition (Edgion extension on GatewayClass; upstream Gateway API
        // defines only Accepted + SupportedVersion here).
        match &ref_state {
            ParamsRefState::Resolved | ParamsRefState::ResolvedInvalid(_) => update_condition(
                &mut status.conditions,
                condition_true(
                    condition_types::RESOLVED_REFS,
                    condition_reasons::RESOLVED_REFS,
                    "parametersRef resolved",
                    generation,
                ),
            ),
            ParamsRefState::Invalid(msg) => update_condition(
                &mut status.conditions,
                condition_false(
                    condition_types::RESOLVED_REFS,
                    condition_reasons::INVALID_PARAMETERS,
                    msg.clone(),
                    generation,
                ),
            ),
            // Nothing to resolve (no parametersRef, or EGC kind not watched): actively remove
            // any previously written ResolvedRefs condition — update_condition never prunes.
            ParamsRefState::Skip => {
                status.conditions.retain(|c| c.type_ != condition_types::RESOLVED_REFS);
            }
        }

        // Accepted condition. Precedence: a parametersRef problem yields reason
        // InvalidParameters; any other validation error keeps reason Invalid; otherwise Accepted.
        let accepted = if let ParamsRefState::Invalid(msg) | ParamsRefState::ResolvedInvalid(msg) = &ref_state {
            condition_false(
                condition_types::ACCEPTED,
                condition_reasons::INVALID_PARAMETERS,
                msg.clone(),
                generation,
            )
        } else if !validation_errors.is_empty() {
            condition_false(
                condition_types::ACCEPTED,
                "Invalid",
                validation_errors.join("; "),
                generation,
            )
        } else {
            condition_true("Accepted", "Accepted", "GatewayClass accepted", generation)
        };
        update_condition(&mut status.conditions, accepted);

        update_condition(
            &mut status.conditions,
            condition_unknown(
                "SupportedVersion",
                "CompatibilityUnknown",
                "Installed Gateway API CRD bundle and channel compatibility has not been verified",
                generation,
            ),
        );

        // Gateway API v1.5 requires supportedFeatures as objects with a `name`
        // field, sorted ascending. The canonical list lives in edgion-resources
        // (SUPPORTED_FEATURES) so the controller status and the Go conformance
        // suite derive from a single source and cannot silently drift; it is
        // already sorted and de-duplicated (guarded by unit tests there).
        status.supported_features = Some(
            SUPPORTED_FEATURES
                .iter()
                .map(|name| SupportedFeature {
                    name: (*name).to_string(),
                })
                .collect(),
        );
    }
}

#[cfg(test)]
#[path = "tests/gateway_class_status_test.rs"]
mod status_tests;

#[cfg(test)]
mod tests {
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
}
