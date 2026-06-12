//! TLSRoute Handler
//!
//! Handles TLSRoute resources with ReferenceGrant validation and cross-namespace reference tracking.

use super::super::ref_grant::{is_cross_ns_ref_allowed, validate_tls_route_if_enabled};
use super::{
    remove_from_attached_route_tracker, remove_from_gateway_route_index, requeue_backend_tls_policies_for_route,
    requeue_parent_gateways, update_gateway_route_index,
};
use crate::conf_mgr::sync_runtime::resource_processor::{
    set_parent_conditions_full, AcceptedError, HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
};
use edgion_resources::prelude_resources::{TLSRoute, TLSRouteAlpha3};
use edgion_resources::resources::common::RefDenied;
use edgion_resources::resources::http_route::RouteParentStatus;
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

    /// Register Service backend references for cross-resource requeue
    fn register_service_refs(route: &TLSRoute, ctx: &HandlerContext) {
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");

        let mut backend_refs_list = Vec::new();
        if let Some(rules) = &route.spec.rules {
            for rule in rules {
                if let Some(backend_refs) = &rule.backend_refs {
                    for br in backend_refs {
                        backend_refs_list.push((
                            br.group.as_deref(),
                            br.kind.as_deref(),
                            br.namespace.as_deref(),
                            br.name.as_str(),
                        ));
                    }
                }
            }
        }

        super::route_utils::register_service_backend_refs(
            ResourceKind::TLSRoute,
            route_ns,
            route_name,
            &backend_refs_list,
            ctx.runtime(),
        );
    }

    /// Record cross-namespace references from backend_refs
    fn record_cross_ns_refs(route: &TLSRoute, ctx: &HandlerContext) {
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let resource_ref = Self::create_resource_ref(route);
        let manager = &ctx.runtime().cross_ns_ref_mgr;

        // Clear old references first (handles updates)
        manager.clear_resource_refs(&resource_ref);

        // Collect cross-namespace references from rules
        if let Some(rules) = &route.spec.rules {
            for rule in rules {
                if let Some(backend_refs) = &rule.backend_refs {
                    for backend_ref in backend_refs {
                        if let Some(backend_ns) = &backend_ref.namespace {
                            if backend_ns != route_ns {
                                manager.add_ref(backend_ns.clone(), resource_ref.clone());
                            }
                        }
                    }
                }
            }
        }
    }
}

impl Default for TlsRouteHandler {
    fn default() -> Self {
        Self::new("edgion.io/gateway-controller".to_string())
    }
}

impl TlsRouteHandler {
    fn validate_route(&self, route: &TLSRoute) -> Vec<String> {
        validate_tls_route_if_enabled(route)
    }

    fn preparse_route(&self, route: &mut TLSRoute, ctx: &HandlerContext) -> Vec<String> {
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");
        update_gateway_route_index(
            ResourceKind::TLSRoute,
            route_ns,
            route_name,
            route.spec.parent_refs.as_ref(),
            ctx.runtime(),
        );
        Vec::new()
    }

    async fn parse_route(&self, mut route: TLSRoute, ctx: &HandlerContext) -> ProcessResult<TLSRoute> {
        Self::record_cross_ns_refs(&route, ctx);
        Self::register_service_refs(&route, ctx);

        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        if let Some(rules) = &mut route.spec.rules {
            for rule in rules {
                if let Some(backend_refs) = &mut rule.backend_refs {
                    for backend_ref in backend_refs {
                        if let Some(backend_ns) = &backend_ref.namespace {
                            if backend_ns != route_ns {
                                let allowed = is_cross_ns_ref_allowed(
                                    route_ns,
                                    "TLSRoute",
                                    backend_ns,
                                    backend_ref.group.as_ref(),
                                    backend_ref.kind.as_ref(),
                                    &backend_ref.name,
                                    ctx.runtime(),
                                );
                                if !allowed {
                                    backend_ref.ref_denied = Some(RefDenied {
                                        target_namespace: backend_ns.clone(),
                                        target_name: backend_ref.name.clone(),
                                        reason: Some("NoMatchingReferenceGrant".to_string()),
                                    });
                                } else {
                                    backend_ref.ref_denied = None;
                                }
                            } else {
                                backend_ref.ref_denied = None;
                            }
                        } else {
                            backend_ref.ref_denied = None;
                        }
                    }
                }
            }
        }

        route.spec.resolved_listeners = None;
        route.spec.effective_hostnames = None;

        if let Some(parent_refs) = &route.spec.parent_refs {
            let (listener_keys, listener_hostnames) = super::route_utils::resolve_listener_keys_with_hostnames(
                parent_refs,
                route_ns,
                route.metadata.namespace.as_deref(),
                ctx.runtime(),
            );
            if !listener_keys.is_empty() {
                route.spec.resolved_listeners = Some(listener_keys);
            }

            let effective = super::route_utils::compute_effective_hostnames(
                route.spec.hostnames.as_deref().unwrap_or_default(),
                &listener_hostnames,
            );
            if !effective.is_empty() {
                route.spec.effective_hostnames = Some(effective);
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

        update_gateway_route_index(
            ResourceKind::TLSRoute,
            route_ns,
            route_name,
            route.spec.parent_refs.as_ref(),
            ctx.runtime(),
        );

        requeue_parent_gateways(route.spec.parent_refs.as_ref(), route_ns, ctx).await;
        requeue_backend_tls_policies_for_route(ResourceKind::TLSRoute, route_ns, route_name, ctx).await;
    }

    async fn on_delete_route(&self, route: &TLSRoute, ctx: &HandlerContext) {
        let resource_ref = Self::create_resource_ref(route);
        ctx.runtime().cross_ns_ref_mgr.clear_resource_refs(&resource_ref);

        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");
        requeue_backend_tls_policies_for_route(ResourceKind::TLSRoute, route_ns, route_name, ctx).await;
        super::route_utils::clear_service_backend_refs(ResourceKind::TLSRoute, route_ns, route_name, ctx.runtime());

        remove_from_gateway_route_index(ResourceKind::TLSRoute, route_ns, route_name, ctx.runtime());

        let tracker_changed =
            remove_from_attached_route_tracker(ResourceKind::TLSRoute, route_ns, route_name, ctx.runtime());
        if tracker_changed {
            requeue_parent_gateways(route.spec.parent_refs.as_ref(), route_ns, ctx).await;
        }
    }

    fn update_route_status(&self, route: &mut TLSRoute, ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = route.metadata.generation;
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");

        let validation_accepted_errors = AcceptedError::from_validation_errors(validation_errors);

        let backend_ref_tuples = collect_backend_ref_tuples(route);
        let mut resolved_refs_errors =
            super::route_utils::validate_backend_refs(route_ns, &backend_ref_tuples, ctx.runtime());
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
                accepted_errors.extend(validation_accepted_errors.clone());

                let parent_status = status.parents.iter_mut().find(|ps| {
                    ps.parent_ref.name == parent_ref.name
                        && ps.parent_ref.namespace == parent_ref.namespace
                        && ps.parent_ref.section_name == parent_ref.section_name
                });

                if let Some(ps) = parent_status {
                    set_parent_conditions_full(&mut ps.conditions, &accepted_errors, &resolved_refs_errors, generation);
                } else {
                    let mut conditions = Vec::new();
                    set_parent_conditions_full(&mut conditions, &accepted_errors, &resolved_refs_errors, generation);

                    status.parents.push(RouteParentStatus {
                        parent_ref: parent_ref.clone(),
                        controller_name: self.controller_name.clone(),
                        conditions,
                    });
                }
            }

            super::route_utils::retain_current_parent_statuses(&mut status.parents, parent_refs);
        } else {
            status.parents.clear();
        }

        super::route_utils::update_tracker_accepted_only(
            ResourceKind::TLSRoute,
            route_ns,
            route.metadata.name.as_deref().unwrap_or(""),
            route.spec.parent_refs.as_ref(),
            route.spec.hostnames.as_ref(),
            ctx.runtime(),
        );
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<TLSRoute> for TlsRouteHandler {
    fn validate(&self, route: &TLSRoute, _ctx: &HandlerContext) -> Vec<String> {
        self.validate_route(route)
    }

    fn preparse(&self, route: &mut TLSRoute, _ctx: &HandlerContext) -> Vec<String> {
        self.preparse_route(route, _ctx)
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
    }
}

// ============================================================================
// TLSRoute v1alpha3 compat handler
// ============================================================================

/// TLSRoute v1alpha3 compat handler.
///
/// Keeps the older watch endpoint working while reusing the same validation and
/// runtime preprocessing logic as the v1 handler.
pub struct TlsRouteAlpha3Handler {
    inner: TlsRouteHandler,
}

impl TlsRouteAlpha3Handler {
    pub fn new(controller_name: String) -> Self {
        Self {
            inner: TlsRouteHandler::new(controller_name),
        }
    }
}

impl Default for TlsRouteAlpha3Handler {
    fn default() -> Self {
        Self::new("edgion.io/gateway-controller".to_string())
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<TLSRouteAlpha3> for TlsRouteAlpha3Handler {
    fn validate(&self, route: &TLSRouteAlpha3, _ctx: &HandlerContext) -> Vec<String> {
        self.inner.validate_route(&route.clone().into())
    }

    fn preparse(&self, route: &mut TLSRouteAlpha3, _ctx: &HandlerContext) -> Vec<String> {
        let mut v1: TLSRoute = route.clone().into();
        let errors = self.inner.preparse_route(&mut v1, _ctx);
        *route = v1.into();
        errors
    }

    async fn parse(&self, route: TLSRouteAlpha3, _ctx: &HandlerContext) -> ProcessResult<TLSRouteAlpha3> {
        match self.inner.parse_route(route.into(), _ctx).await {
            ProcessResult::Continue(v1) => ProcessResult::Continue(v1.into()),
            ProcessResult::Skip { reason } => ProcessResult::Skip { reason },
        }
    }

    async fn on_change(&self, route: &TLSRouteAlpha3, ctx: &HandlerContext) {
        let v1: TLSRoute = route.clone().into();
        self.inner.on_change_route(&v1, ctx).await;
    }

    async fn on_delete(&self, route: &TLSRouteAlpha3, ctx: &HandlerContext) {
        let v1: TLSRoute = route.clone().into();
        self.inner.on_delete_route(&v1, ctx).await;
    }

    fn update_status(&self, route: &mut TLSRouteAlpha3, _ctx: &HandlerContext, validation_errors: &[String]) {
        let mut v1: TLSRoute = route.clone().into();
        self.inner.update_route_status(&mut v1, _ctx, validation_errors);
        *route = v1.into();
    }
}

fn collect_backend_ref_tuples(route: &TLSRoute) -> Vec<(Option<&str>, Option<&str>, Option<&str>, &str)> {
    let mut tuples = Vec::new();
    if let Some(rules) = &route.spec.rules {
        for rule in rules {
            if let Some(backend_refs) = &rule.backend_refs {
                for br in backend_refs {
                    tuples.push((
                        br.group.as_deref(),
                        br.kind.as_deref(),
                        br.namespace.as_deref(),
                        br.name.as_str(),
                    ));
                }
            }
        }
    }
    tuples
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
