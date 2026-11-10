//! GatewayClass Handler
//!
//! Handles GatewayClass resources.

#[cfg(test)]
use crate::conf_mgr::conf_center::common::DEFAULT_CONTROLLER_NAME;
use crate::conf_mgr::sync_runtime::resource_processor::credential::egc_tls;
use crate::conf_mgr::sync_runtime::resource_processor::status_ownership::gateway_has_class_owner;
use crate::conf_mgr::sync_runtime::resource_processor::status_utils::condition_unknown;
use crate::conf_mgr::sync_runtime::resource_processor::{
    condition_false, condition_reasons, condition_true, condition_types, update_condition, HandlerContext,
    ProcessResult, ProcessorHandler, ResolvedRefsError,
};
use crate::conf_mgr::ControllerRuntime;
use crate::config::is_reference_grant_validation_enabled;
use edgion_resources::prelude_resources::{EdgionGatewayConfig, Gateway, GatewayClass};
use edgion_resources::refs::condition_fields::PreparedConditionReferences;
use edgion_resources::resources::gateway_class::{GatewayClassStatus, SupportedFeature, SUPPORTED_FEATURES};
use edgion_resources::resources::refs::gateway_class_fields::GatewayClassParametersRef;
use edgion_resources::resources::refs::plugin_attachment_fields::HttpPluginAttachments;
use edgion_resources::resources::refs::plugin_target::PluginTarget;
use edgion_resources::resources::refs::ReferenceError;
#[cfg(test)]
use edgion_resources::EdgionPlugins;

/// Outcome of resolving `GatewayClass.spec.parametersRef`.
enum ParamsRefState {
    /// No parametersRef, or the EdgionGatewayConfig kind is not watched — nothing to assert.
    Skip,
    /// parametersRef present, group/kind valid, and the target EdgionGatewayConfig exists.
    /// `invalid` holds deployment-local early plugin attachment errors (Accepted);
    /// `unresolved` lists EGC references this deployment cannot resolve (ResolvedRefs).
    Resolved {
        invalid: Option<String>,
        unresolved: Vec<String>,
    },
    /// parametersRef present but unusable: bad group/kind, or target not found.
    Invalid(String),
}

/// Resolve `parametersRef` synchronously against the processor registry.
///
/// EdgionGatewayConfig is cluster-scoped, so its cache key is the bare `name`. The lookup
/// reads the RawStore, which the watch/file layer populates upstream of the processor
/// pipeline, so a GatewayClass requeued by an EGC change always observes the current state.
fn resolve_params_ref(gc: &GatewayClass, ctx: &HandlerContext) -> ParamsRefState {
    let reference = match GatewayClassParametersRef::from_class(gc) {
        Ok(None) => return ParamsRefState::Skip,
        Ok(Some(reference)) => reference,
        Err(ReferenceError::UnsupportedTarget) => {
            let Some(field) = gc.spec.parameters_ref.as_ref() else {
                return ParamsRefState::Invalid("parametersRef: unsupported reference target".into());
            };
            return ParamsRefState::Invalid(format!(
                "parametersRef must reference edgion.io/EdgionGatewayConfig, got \"{}/{}\"",
                field.group, field.kind
            ));
        }
        Err(error) => return ParamsRefState::Invalid(format!("parametersRef: {error}")),
    };
    let target = reference.target();
    match ctx.runtime().processor_registry.get("EdgionGatewayConfig") {
        // EGC kind not watched (no_watch_kinds opt-out): existence unknown, do not assert.
        None => ParamsRefState::Skip,
        Some(processor) => {
            let Some(egc) = processor
                .get_raw_any(target.name())
                .and_then(|raw| raw.downcast::<EdgionGatewayConfig>().ok())
            else {
                return ParamsRefState::Invalid(format!(
                    "referenced EdgionGatewayConfig \"{}\" not found",
                    target.name()
                ));
            };
            // Unlike the shared cluster-scoped EGC, this class is owned by one
            // deployment and may report that deployment's local plugin validation
            // and reference availability. What this deployment cannot see, it
            // cannot ship to its Gateways, so missing means locally unusable.
            let mut errors = Vec::new();
            let mut unresolved = egc_tls::unavailable_material(ctx, &egc);
            let plugins_watched = ctx.runtime().processor_registry.get("EdgionPlugins").is_some();
            for (index, reference) in HttpPluginAttachments::EgcGlobals(&egc)
                .entries()
                .into_iter()
                .enumerate()
            {
                let key = match reference {
                    Ok(reference) => reference.into_key(),
                    Err(error) => {
                        errors.push(format!("globalPluginsRef[{index}]: {error}"));
                        continue;
                    }
                };
                if !plugins_watched {
                    continue;
                }
                match super::route_utils::plugin_target(ctx.runtime(), &key) {
                    PluginTarget::Unique { source: plugins, .. } => errors.extend(
                        plugins
                            .validate_request_id_early_attachment()
                            .into_iter()
                            .map(|error| format!("globalPluginsRef {key}: {error}")),
                    ),
                    PluginTarget::Ambiguous => errors.push(format!("globalPluginsRef {key} is ambiguous")),
                    PluginTarget::Missing => unresolved.push(format!("globalPluginsRef {key} not found")),
                }
            }
            unresolved.extend(unavailable_trusted_ip_lists(&egc, ctx));
            ParamsRefState::Resolved {
                invalid: (!errors.is_empty()).then(|| errors.join("; ")),
                unresolved,
            }
        }
    }
}

/// Trusted RealIp lists the global EGC policy cannot read in this deployment.
/// Intrinsic reference errors are skipped; shared EGC Accepted reports them.
fn unavailable_trusted_ip_lists(egc: &EdgionGatewayConfig, ctx: &HandlerContext) -> Vec<String> {
    let mut unavailable = Vec::new();
    let trust = PreparedConditionReferences::from_gateway_config(egc);
    let _ = trust.visit_gateway_ip(egc, |leaf| {
        let Ok(tokens) = leaf.references() else {
            return;
        };
        for (index, token) in tokens.enumerate() {
            let mut present = false;
            let problem = match super::ip_list_refs::resolve_ip_reference(
                token,
                is_reference_grant_validation_enabled(),
                |namespace, name| {
                    let data = ctx.runtime().edgion_config_data_store.get(Some(namespace), name);
                    present = data.is_some();
                    data
                },
                // A cluster owner never consults a namespaced ReferenceGrant.
                |_| false,
            )
            .error
            {
                None | Some(ResolvedRefsError::InvalidReference { .. }) => continue,
                Some(ResolvedRefsError::RefNotPermitted {
                    target_namespace,
                    target_name,
                    ..
                }) => format!(
                    "EdgionConfigData '{target_namespace}/{target_name}' must have Cluster visibility for a cluster-scoped reference"
                ),
                // The shared resolver reports missing, wrong-type and disabled
                // lists alike; tell the operator which one it is.
                Some(ResolvedRefsError::EdgionConfigDataNotFound { namespace, name, .. }) => {
                    if present {
                        format!("EdgionConfigData '{namespace}/{name}' is not an enabled IpList")
                    } else {
                        format!("EdgionConfigData '{namespace}/{name}' not found")
                    }
                }
                Some(error) => error.message(),
            };
            unavailable.push(format!("realIp.trustedIpRefs[{index}]: {problem}"));
        }
    });
    unavailable
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
            ParamsRefState::Resolved { unresolved, .. } if unresolved.is_empty() => update_condition(
                &mut status.conditions,
                condition_true(
                    condition_types::RESOLVED_REFS,
                    condition_reasons::RESOLVED_REFS,
                    "parametersRef resolved",
                    generation,
                ),
            ),
            ParamsRefState::Resolved { unresolved, .. } => update_condition(
                &mut status.conditions,
                condition_false(
                    condition_types::RESOLVED_REFS,
                    condition_reasons::INVALID_PARAMETERS,
                    format!(
                        "referenced EdgionGatewayConfig has unresolved references: {}",
                        unresolved.join("; ")
                    ),
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
        let accepted =
            if let ParamsRefState::Invalid(msg) | ParamsRefState::Resolved { invalid: Some(msg), .. } = &ref_state {
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
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/gateway_class_test/status_tests.rs"]
mod status_tests;

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/gateway_class_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../../tests/support/conf_mgr/sync_runtime/resource_processor/handlers/gateway_class.rs"]
mod test_support;
