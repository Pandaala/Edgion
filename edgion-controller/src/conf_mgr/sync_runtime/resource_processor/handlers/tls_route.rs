//! TLSRoute Handler
//!
//! Handles TLSRoute resources with ReferenceGrant validation and cross-namespace reference tracking.

use super::super::ref_grant::validate_tls_route_if_enabled;
use super::route_backend_refs;
use super::{
    remove_from_attached_route_tracker, remove_from_gateway_route_index, requeue_backend_policies_for_owner,
    requeue_backend_policies_for_services, requeue_parent_gateways, update_gateway_route_index,
};
use crate::conf_mgr::sync_runtime::resource_processor::{
    set_parent_conditions_full, AcceptedError, HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
};
use edgion_resources::prelude_resources::TLSRoute;
use edgion_resources::resources::common::RefDenied;
use edgion_resources::resources::refs::route_fields::TlsRouteBackendRefs;
use edgion_resources::resources::tls_route::TLSRouteStatus;
use edgion_resources::ResourceKind;

/// TLSRoute handler
pub struct TlsRouteHandler {
    controller_name: String,
}

impl TlsRouteHandler {
    pub fn new(controller_name: String) -> Self {
        Self { controller_name }
    }

    fn create_resource_ref(route: &TLSRoute) -> ResourceRef {
        ResourceRef::new(
            ResourceKind::TLSRoute,
            route.metadata.namespace.clone(),
            route.metadata.name.clone().unwrap_or_default(),
        )
    }

    /// Register Service backend references for cross-resource requeue.
    /// Returns the Service keys dropped from backendRefs by this update.
    fn register_service_refs(route: &TLSRoute, ctx: &HandlerContext) -> Vec<String> {
        let backend_refs_list = TlsRouteBackendRefs::from_route(route)
            .map(|references| route_backend_refs::collect_dependencies(references.authored()))
            .unwrap_or_default();

        super::route_utils::register_service_backend_refs(
            &Self::create_resource_ref(route),
            &backend_refs_list,
            ctx.runtime(),
        )
    }

    /// Record cross-namespace references from backend_refs
    fn record_cross_ns_refs(route: &TLSRoute, ctx: &HandlerContext) {
        let resource_ref = Self::create_resource_ref(route);
        ctx.runtime().cross_ns_ref_mgr.clear_value_refs(&resource_ref);
        let receipts = TlsRouteBackendRefs::from_route(route)
            .map(|references| route_backend_refs::collect_dependencies(references.authored()))
            .unwrap_or_default();
        route_backend_refs::register_cross_namespace(&receipts, &resource_ref, ctx.runtime());
    }
}

impl Default for TlsRouteHandler {
    fn default() -> Self {
        Self::new("edgion.io/gateway-controller".to_string())
    }
}

impl TlsRouteHandler {
    fn validate_route(&self, route: &TLSRoute) -> Vec<String> {
        // Hostname errors are classified separately as UnsupportedValue in status.
        let mut errors = validate_tls_route_if_enabled(route);
        if let Err(error) = TlsRouteBackendRefs::from_route(route) {
            errors.push(route_backend_refs::source_error_message(error));
        }
        errors
    }

    async fn parse_route(&self, mut route: TLSRoute, ctx: &HandlerContext) -> ProcessResult<TLSRoute> {
        let source_valid = TlsRouteBackendRefs::from_route(&route).is_ok();
        // Install new parent edges before reading a Gateway snapshot, so a
        // concurrent Gateway publication cannot miss this route. on_change
        // removes old edges and requeues dropped parents after tracker updates.
        if source_valid {
            ctx.runtime().gateway_route_index.register_route_dependencies(
                ResourceKind::TLSRoute,
                &format!(
                    "{}/{}",
                    route.metadata.namespace.as_deref().unwrap_or("default"),
                    route.metadata.name.as_deref().unwrap_or("")
                ),
                route.spec.parent_refs.as_deref().unwrap_or_default(),
                route.metadata.namespace.as_deref().unwrap_or("default"),
            );
        }
        Self::record_cross_ns_refs(&route, ctx);
        // Register Service backend references, then requeue BTPs bound to any
        // Service dropped from backendRefs (the apply path otherwise only
        // requeues BTPs for the route's current backends).
        let dropped_service_keys = Self::register_service_refs(&route, ctx);
        requeue_backend_policies_for_services(&dropped_service_keys, ctx).await;

        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let decisions = TlsRouteBackendRefs::from_route(&route)
            .map(|references| {
                route_backend_refs::resolve_denials(
                    references.authored(),
                    &Self::create_resource_ref(&route),
                    ctx.runtime(),
                )
            })
            .unwrap_or_default();
        for rule in route.spec.rules.iter_mut().flatten() {
            for backend in rule.backend_refs.iter_mut().flatten() {
                backend.ref_denied = None;
            }
        }
        for (slot, denial) in decisions {
            if let Some(backend) = route
                .spec
                .rules
                .as_mut()
                .and_then(|rules| rules.get_mut(slot.rule()))
                .and_then(|rule| rule.backend_refs.as_mut())
                .and_then(|backends| backends.get_mut(slot.backend()))
            {
                backend.ref_denied = denial;
            }
        }

        route.spec.resolved_listeners = None;

        if let Some(parent_refs) = route
            .spec
            .parent_refs
            .as_ref()
            .filter(|_| source_valid && route.spec.validate_hostnames().is_ok())
        {
            let listeners = super::route_utils::resolve_tls_listener_hostnames(
                parent_refs,
                route_ns,
                route.spec.hostnames.as_deref().unwrap_or_default(),
                ctx.runtime(),
            );
            if !listeners.is_empty() {
                route.spec.resolved_listeners = Some(listeners);
            }
        }

        if route.spec.resolved_listeners.is_none() {
            tracing::debug!(
                route = %route.metadata.name.as_deref().unwrap_or(""),
                ns = route_ns,
                "TLSRoute has no resolved_listeners: Gateway not yet available or no matching listeners"
            );
        }

        ProcessResult::Continue(route)
    }

    async fn on_change_route(&self, route: &TLSRoute, ctx: &HandlerContext) {
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");

        let dropped_gateways = update_gateway_route_index(
            ResourceKind::TLSRoute,
            route_ns,
            route_name,
            route.spec.parent_refs.as_ref(),
            ctx.runtime(),
        );
        for gateway_key in dropped_gateways {
            ctx.requeue("Gateway", gateway_key).await;
        }

        requeue_parent_gateways(route.spec.parent_refs.as_ref(), route_ns, ctx).await;
        requeue_backend_policies_for_owner(&Self::create_resource_ref(route), ctx).await;
    }

    async fn on_delete_route(&self, route: &TLSRoute, ctx: &HandlerContext) {
        let resource_ref = Self::create_resource_ref(route);
        ctx.runtime().cross_ns_ref_mgr.clear_value_refs(&resource_ref);

        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");
        requeue_backend_policies_for_owner(&Self::create_resource_ref(route), ctx).await;
        super::route_utils::clear_service_backend_refs(&resource_ref, ctx.runtime());

        remove_from_gateway_route_index(ResourceKind::TLSRoute, route_ns, route_name, ctx.runtime());

        let tracker_changed =
            remove_from_attached_route_tracker(ResourceKind::TLSRoute, route_ns, route_name, ctx.runtime());
        if tracker_changed {
            requeue_parent_gateways(route.spec.parent_refs.as_ref(), route_ns, ctx).await;
        }
    }

    fn update_route_status(&self, route: &mut TLSRoute, ctx: &HandlerContext, validation_errors: &[String]) {
        if TlsRouteBackendRefs::from_route(route).is_err() {
            route.spec.resolved_listeners = None;
        }
        let generation = route.metadata.generation;
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");

        let hostname_error = route.spec.validate_hostnames().err();
        let mut validation_accepted_errors = AcceptedError::from_validation_errors(validation_errors);
        if let Err(error) = TlsRouteBackendRefs::from_route(route) {
            let message = route_backend_refs::source_error_message(error);
            if !validation_errors.contains(&message) {
                validation_accepted_errors.push(AcceptedError::Invalid { message });
            }
        }

        let mut resolved_refs_errors = route_backend_refs::validate_targets(
            TlsRouteBackendRefs::from_route(route).map(|references| references.effective()),
            ctx.runtime(),
        );
        let ref_denied_list = collect_ref_denied_list(route);
        resolved_refs_errors.extend(super::route_utils::collect_ref_denied_errors(
            route_ns,
            ResourceKind::TLSRoute,
            &ref_denied_list,
        ));

        let status = route.status.get_or_insert_with(|| TLSRouteStatus { parents: vec![] });
        let route_hostnames = route.spec.hostnames.as_ref();

        if let Some(parent_refs) = &route.spec.parent_refs {
            for parent_ref in parent_refs {
                let accepted_result = super::route_utils::validate_parent_ref_accepted(
                    route_ns,
                    parent_ref,
                    route_hostnames,
                    "TLSRoute",
                    ctx.runtime(),
                );
                let mut accepted_errors = accepted_result.errors;
                if let Some(error) = hostname_error {
                    accepted_errors.insert(
                        0,
                        AcceptedError::UnsupportedValue {
                            message: error.to_string(),
                        },
                    );
                }
                accepted_errors.extend(validation_accepted_errors.clone());

                let ps = super::route_utils::parent_status_for_update(
                    &mut status.parents,
                    parent_ref,
                    route_ns,
                    &self.controller_name,
                );
                set_parent_conditions_full(&mut ps.conditions, &accepted_errors, &resolved_refs_errors, generation);
            }

            super::route_utils::retain_current_parent_statuses(
                &mut status.parents,
                parent_refs,
                route_ns,
                &self.controller_name,
            );
        } else {
            super::route_utils::retain_current_parent_statuses(
                &mut status.parents,
                &[],
                route_ns,
                &self.controller_name,
            );
        }

        // Globally rejected routes must not contribute to attachedRoutes.
        // Passing no parents also removes attachments from the prior generation.
        let accepted_parents = route.spec.parent_refs.as_ref().filter(|_| {
            TlsRouteBackendRefs::from_route(route).is_ok() && validation_errors.is_empty() && hostname_error.is_none()
        });
        super::route_utils::update_tracker_accepted_only(
            ResourceKind::TLSRoute,
            route_ns,
            route.metadata.name.as_deref().unwrap_or(""),
            accepted_parents,
            route.spec.hostnames.as_ref(),
            ctx.runtime(),
        );
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<TLSRoute> for TlsRouteHandler {
    fn finalize_transport(&self, route: &mut TLSRoute, _ctx: &HandlerContext) {
        route.spec.resolved_status_controller = Some(self.controller_name.clone());
    }

    fn validate(&self, route: &TLSRoute, _ctx: &HandlerContext) -> Vec<String> {
        self.validate_route(route)
    }

    async fn parse(&self, route: TLSRoute, _ctx: &HandlerContext) -> ProcessResult<TLSRoute> {
        self.parse_route(route, _ctx).await
    }

    async fn on_change(&self, route: &TLSRoute, ctx: &HandlerContext) {
        self.on_change_route(route, ctx).await;
    }

    async fn on_delete(&self, route: &TLSRoute, ctx: &HandlerContext) {
        self.on_delete_route(route, ctx).await;
    }

    fn update_status(&self, route: &mut TLSRoute, _ctx: &HandlerContext, validation_errors: &[String]) {
        self.update_route_status(route, _ctx, validation_errors);
        super::policy_ancestor_admission::annotate(route, &self.controller_name, _ctx);
    }
}

fn collect_ref_denied_list(route: &TLSRoute) -> Vec<Option<&RefDenied>> {
    let mut list = Vec::new();
    if let Some(rules) = &route.spec.rules {
        for rule in rules {
            if let Some(backend_refs) = &rule.backend_refs {
                for br in backend_refs {
                    list.push(br.ref_denied.as_ref());
                }
            }
        }
    }
    list
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/tls_route_test.rs"]
mod tests;
