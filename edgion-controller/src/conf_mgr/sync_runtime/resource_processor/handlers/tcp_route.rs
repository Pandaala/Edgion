//! TCPRoute Handler
//!
//! Handles TCPRoute resources with ReferenceGrant validation and cross-namespace reference tracking.

use super::super::ref_grant::validate_tcp_route_if_enabled;
use super::route_backend_refs;
use super::{
    remove_from_attached_route_tracker, remove_from_gateway_route_index, requeue_backend_policies_for_owner,
    requeue_backend_policies_for_services, requeue_parent_gateways, update_gateway_route_index,
};
use crate::conf_mgr::sync_runtime::resource_processor::{
    set_parent_conditions_full, AcceptedError, HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
};
use edgion_resources::prelude_resources::TCPRoute;
use edgion_resources::resources::common::RefDenied;
use edgion_resources::resources::refs::route_fields::TcpRouteBackendRefs;
use edgion_resources::resources::tcp_route::TCPRouteStatus;
use edgion_resources::ResourceKind;

/// TCPRoute handler
pub struct TcpRouteHandler {
    controller_name: String,
}

impl TcpRouteHandler {
    pub fn new(controller_name: String) -> Self {
        Self { controller_name }
    }

    fn create_resource_ref(route: &TCPRoute) -> ResourceRef {
        ResourceRef::new(
            ResourceKind::TCPRoute,
            route.metadata.namespace.clone(),
            route.metadata.name.clone().unwrap_or_default(),
        )
    }

    /// Register Service backend references for cross-resource requeue.
    /// Returns the Service keys dropped from backendRefs by this update.
    fn register_service_refs(route: &TCPRoute, ctx: &HandlerContext) -> Vec<String> {
        let backend_refs_list = TcpRouteBackendRefs::from_route(route)
            .map(|references| route_backend_refs::collect_dependencies(references.authored()))
            .unwrap_or_default();

        super::route_utils::register_service_backend_refs(
            &Self::create_resource_ref(route),
            &backend_refs_list,
            ctx.runtime(),
        )
    }

    /// Record cross-namespace references from backend_refs
    fn record_cross_ns_refs(route: &TCPRoute, ctx: &HandlerContext) {
        let resource_ref = Self::create_resource_ref(route);
        ctx.runtime().cross_ns_ref_mgr.clear_value_refs(&resource_ref);
        let receipts = TcpRouteBackendRefs::from_route(route)
            .map(|references| route_backend_refs::collect_dependencies(references.authored()))
            .unwrap_or_default();
        route_backend_refs::register_cross_namespace(&receipts, &resource_ref, ctx.runtime());
    }
}

impl Default for TcpRouteHandler {
    fn default() -> Self {
        Self::new("edgion.io/gateway-controller".to_string())
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<TCPRoute> for TcpRouteHandler {
    fn finalize_transport(&self, route: &mut TCPRoute, _ctx: &HandlerContext) {
        route.spec.resolved_status_controller = Some(self.controller_name.clone());
    }

    fn validate(&self, route: &TCPRoute, _ctx: &HandlerContext) -> Vec<String> {
        let mut errors = validate_tcp_route_if_enabled(route);
        if let Err(error) = TcpRouteBackendRefs::from_route(route) {
            errors.push(route_backend_refs::source_error_message(error));
        }
        errors
    }

    fn should_publish(&self, route: &TCPRoute, _ctx: &HandlerContext) -> bool {
        validate_tcp_route_if_enabled(route).is_empty()
    }

    async fn parse(&self, mut route: TCPRoute, _ctx: &HandlerContext) -> ProcessResult<TCPRoute> {
        let source_valid = TcpRouteBackendRefs::from_route(&route).is_ok();
        if source_valid && !validate_tcp_route_if_enabled(&route).is_empty() {
            return ProcessResult::Continue(route);
        }
        // Record cross-namespace references for revalidation when ReferenceGrant changes
        Self::record_cross_ns_refs(&route, _ctx);

        // Register Service backend references for cross-resource requeue, then
        // requeue BTPs bound to any Service dropped from backendRefs (the apply
        // path otherwise only requeues BTPs for the route's current backends).
        let dropped_service_keys = Self::register_service_refs(&route, _ctx);
        requeue_backend_policies_for_services(&dropped_service_keys, _ctx).await;

        // Mark denied cross-namespace references
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let decisions = TcpRouteBackendRefs::from_route(&route)
            .map(|references| {
                route_backend_refs::resolve_denials(
                    references.authored(),
                    &Self::create_resource_ref(&route),
                    _ctx.runtime(),
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

        // Treat resolved_listeners as controller-derived state.
        // Clear any stale value before recomputing from current parentRefs.
        route.spec.resolved_listeners = None;
        route.spec.resolved_listener_attachments = None;

        // Resolve listener keys from parentRefs for per-listener route isolation
        if let Some(parent_refs) = route.spec.parent_refs.as_ref().filter(|_| source_valid) {
            let attachments =
                super::route_utils::resolve_route_listener_attachments(parent_refs, route_ns, _ctx.runtime());
            if !attachments.is_empty() {
                let mut keys: Vec<_> = attachments
                    .iter()
                    .map(|attachment| attachment.listener_key.clone())
                    .collect();
                keys.sort();
                keys.dedup();
                route.spec.resolved_listeners = Some(keys);
                route.spec.resolved_listener_attachments = Some(attachments);
            }
        }

        if route.spec.resolved_listeners.is_none() {
            tracing::debug!(
                route = %route.metadata.name.as_deref().unwrap_or(""),
                ns = route_ns,
                "TCPRoute has no resolved_listeners: Gateway not yet available or no matching listeners"
            );
        }

        ProcessResult::Continue(route)
    }

    async fn on_change(&self, route: &TCPRoute, ctx: &HandlerContext) {
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");

        // Register in gateway_route_index so Gateway changes trigger requeue
        // (required because parse() calls lookup_gateway() for resolved_listeners)
        let dropped_gateways = update_gateway_route_index(
            ResourceKind::TCPRoute,
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

    async fn on_delete(&self, route: &TCPRoute, ctx: &HandlerContext) {
        let resource_ref = Self::create_resource_ref(route);
        ctx.runtime().cross_ns_ref_mgr.clear_value_refs(&resource_ref);

        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");
        requeue_backend_policies_for_owner(&Self::create_resource_ref(route), ctx).await;
        super::route_utils::clear_service_backend_refs(&resource_ref, ctx.runtime());

        remove_from_gateway_route_index(ResourceKind::TCPRoute, route_ns, route_name, ctx.runtime());

        let tracker_changed =
            remove_from_attached_route_tracker(ResourceKind::TCPRoute, route_ns, route_name, ctx.runtime());
        if tracker_changed {
            requeue_parent_gateways(route.spec.parent_refs.as_ref(), route_ns, ctx).await;
        }
    }

    fn update_status(&self, route: &mut TCPRoute, _ctx: &HandlerContext, validation_errors: &[String]) {
        if TcpRouteBackendRefs::from_route(route).is_err() {
            route.spec.resolved_listeners = None;
            route.spec.resolved_listener_attachments = None;
        }
        let generation = route.metadata.generation;
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");

        let mut validation_accepted_errors = AcceptedError::from_validation_errors(validation_errors);
        if let Err(error) = TcpRouteBackendRefs::from_route(route) {
            let message = route_backend_refs::source_error_message(error);
            if !validation_errors.contains(&message) {
                validation_accepted_errors.push(AcceptedError::Invalid { message });
            }
        }

        let mut resolved_refs_errors = route_backend_refs::validate_targets(
            TcpRouteBackendRefs::from_route(route).map(|references| references.effective()),
            _ctx.runtime(),
        );
        let ref_denied_list = collect_ref_denied_list(route);
        resolved_refs_errors.extend(super::route_utils::collect_ref_denied_errors(
            route_ns,
            ResourceKind::TCPRoute,
            &ref_denied_list,
        ));

        let status = route.status.get_or_insert_with(|| TCPRouteStatus { parents: vec![] });

        if let Some(parent_refs) = &route.spec.parent_refs {
            for parent_ref in parent_refs {
                let accepted_result = super::route_utils::validate_parent_ref_accepted(
                    route_ns,
                    parent_ref,
                    None,
                    "TCPRoute",
                    _ctx.runtime(),
                );
                let mut accepted_errors = accepted_result.errors;
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

        if validation_errors.is_empty() || TcpRouteBackendRefs::from_route(route).is_err() {
            super::route_utils::update_tracker_accepted_only(
                ResourceKind::TCPRoute,
                route_ns,
                route.metadata.name.as_deref().unwrap_or(""),
                route
                    .spec
                    .parent_refs
                    .as_ref()
                    .filter(|_| TcpRouteBackendRefs::from_route(route).is_ok()),
                None,
                _ctx.runtime(),
            );
        }
    }
}

fn collect_ref_denied_list(route: &TCPRoute) -> Vec<Option<&RefDenied>> {
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
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/tcp_route_test/coalesced_recreate_backend_tls_test.rs"]
mod coalesced_recreate_backend_tls_test;
