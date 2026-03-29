//! GatewayClass Handler
//!
//! Handles GatewayClass resources.

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    condition_false, condition_true, condition_types, update_condition, HandlerContext, ProcessResult, ProcessorHandler,
};
use crate::types::prelude_resources::GatewayClass;
use crate::types::resources::gateway_class::{GatewayClassStatus, SupportedFeature};

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

        // Gateway API v1.5 requires supportedFeatures as objects with `name` field.
        // List MUST be sorted in ascending alphabetical order by name.
        let mut features: Vec<&str> = vec![
            "Gateway",
            "GRPCRoute",
            "GatewayFrontendClientCertificateValidation",
            "GatewayHTTPListenerIsolation",
            "GatewayPort8080",
            "HTTPRoute",
            "HTTPRouteBackendProtocolH2C",
            "HTTPRouteBackendProtocolWebSocket",
            "HTTPRouteBackendRequestHeaderModification",
            "HTTPRouteBackendTimeout",
            "HTTPRouteHostRewrite",
            "HTTPRouteMethodMatching",
            "HTTPRouteNamedRouteRule",
            "HTTPRouteParentRefPort",
            "HTTPRoutePathRedirect",
            "HTTPRoutePathRewrite",
            "HTTPRoutePortRedirect",
            "HTTPRouteQueryParamMatching",
            "HTTPRouteRequestMirror",
            "HTTPRouteRequestMultipleMirrors",
            "HTTPRouteRequestPercentageMirror",
            "HTTPRouteRequestTimeout",
            "HTTPRouteResponseHeaderModification",
            "HTTPRouteSchemeRedirect",
            "ReferenceGrant",
            "TCPRoute",
            "TLSRoute",
            "UDPRoute",
        ];
        features.sort();
        status.supported_features = Some(
            features
                .into_iter()
                .map(|name| SupportedFeature { name: name.to_string() })
                .collect(),
        );
    }
}
