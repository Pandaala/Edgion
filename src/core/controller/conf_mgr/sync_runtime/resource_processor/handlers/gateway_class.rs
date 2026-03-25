//! GatewayClass Handler
//!
//! Handles GatewayClass resources.

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    condition_false, condition_true, condition_types, update_condition, HandlerContext, ProcessResult, ProcessorHandler,
};
use crate::types::prelude_resources::GatewayClass;
use crate::types::resources::gateway_class::GatewayClassStatus;

/// GatewayClass handler
pub struct GatewayClassHandler {
    controller_name: String,
}

impl GatewayClassHandler {
    pub fn new(controller_name: String) -> Self {
        Self { controller_name }
    }
}

impl Default for GatewayClassHandler {
    fn default() -> Self {
        Self::new("edgion.io/gateway-controller".to_string())
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<GatewayClass> for GatewayClassHandler {
    fn filter(&self, gc: &GatewayClass) -> bool {
        gc.spec.controller_name == self.controller_name
    }

    fn validate(&self, gc: &GatewayClass, _ctx: &HandlerContext) -> Vec<String> {
        let mut errors = Vec::new();
        // Validate parametersRef if present: must reference EdgionGatewayConfig
        if let Some(ref params_ref) = gc.spec.parameters_ref {
            if params_ref.group != "edgion.io" {
                errors.push(format!(
                    "parametersRef.group must be \"edgion.io\", got \"{}\"",
                    params_ref.group
                ));
            }
            if params_ref.kind != "EdgionGatewayConfig" {
                errors.push(format!(
                    "parametersRef.kind must be \"EdgionGatewayConfig\", got \"{}\"",
                    params_ref.kind
                ));
            }
        }
        // parametersRef is optional — when absent, Gateway uses built-in defaults
        errors
    }

    async fn parse(&self, gc: GatewayClass, _ctx: &HandlerContext) -> ProcessResult<GatewayClass> {
        ProcessResult::Continue(gc)
    }

    fn update_status(&self, gc: &mut GatewayClass, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = gc.metadata.generation;
        let status = gc.status.get_or_insert_with(GatewayClassStatus::default);

        if validation_errors.is_empty() {
            update_condition(
                &mut status.conditions,
                condition_true("Accepted", "Accepted", "GatewayClass accepted", generation),
            );
        } else {
            update_condition(
                &mut status.conditions,
                condition_false(
                    condition_types::ACCEPTED,
                    "Invalid",
                    validation_errors.join("; "),
                    generation,
                ),
            );
        }

        update_condition(
            &mut status.conditions,
            condition_true(
                "SupportedVersion",
                "SupportedVersion",
                "Gateway API version is supported",
                generation,
            ),
        );

        status.supported_features = Some(vec![
            "Gateway".to_string(),
            "HTTPRoute".to_string(),
            "ReferenceGrant".to_string(),
            "HTTPRouteQueryParamMatching".to_string(),
            "HTTPRouteMethodMatching".to_string(),
            "HTTPRouteResponseHeaderModification".to_string(),
            "HTTPRouteHostRewrite".to_string(),
            "HTTPRoutePathRewrite".to_string(),
            "HTTPRouteRequestMirror".to_string(),
            "HTTPRouteRequestMultipleMirrors".to_string(),
            "HTTPRouteRequestTimeout".to_string(),
            "HTTPRouteBackendTimeout".to_string(),
            "HTTPRouteBackendRequestHeaderModification".to_string(),
            "HTTPRouteBackendProtocolH2C".to_string(),
            "HTTPRouteBackendProtocolWebSocket".to_string(),
            "HTTPRouteNamedRouteRule".to_string(),
            "GatewayHTTPListenerIsolation".to_string(),
            "GatewayPort8080".to_string(),
            "HTTPRoutePathRedirect".to_string(),
            "HTTPRoutePortRedirect".to_string(),
            "HTTPRouteSchemeRedirect".to_string(),
            "HTTPRouteRequestPercentageMirror".to_string(),
            "HTTPRouteParentRefPort".to_string(),
            "GRPCRoute".to_string(),
            "TLSRoute".to_string(),
            "TCPRoute".to_string(),
            "UDPRoute".to_string(),
            "GatewayFrontendClientCertificateValidation".to_string(),
        ]);
    }
}
