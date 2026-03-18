//! TLSRoute Handler
//!
//! Handles TLSRoute resources with ReferenceGrant validation and cross-namespace reference tracking.

use super::super::ref_grant::{
    get_global_cross_ns_ref_manager, is_cross_ns_ref_allowed, validate_tls_route_if_enabled,
};
use super::{
    remove_from_attached_route_tracker, remove_from_gateway_route_index, requeue_parent_gateways,
    update_attached_route_tracker, update_gateway_route_index,
};
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    set_parent_conditions_full, AcceptedError, HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
};
use crate::types::prelude_resources::TLSRoute;
use crate::types::resources::common::RefDenied;
use crate::types::resources::http_route::RouteParentStatus;
use crate::types::resources::tls_route::TLSRouteStatus;
use crate::types::ResourceKind;

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
    fn register_service_refs(route: &TLSRoute) {
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");

        let mut backend_refs_list = Vec::new();
        if let Some(rules) = &route.spec.rules {
            for rule in rules {
                if let Some(backend_refs) = &rule.backend_refs {
                    for br in backend_refs {
                        backend_refs_list.push((br.kind.as_deref(), br.namespace.as_deref(), br.name.as_str()));
                    }
                }
            }
        }

        super::route_utils::register_service_backend_refs(
            ResourceKind::TLSRoute,
            route_ns,
            route_name,
            &backend_refs_list,
        );
    }

    /// Record cross-namespace references from backend_refs
    fn record_cross_ns_refs(route: &TLSRoute) {
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let resource_ref = Self::create_resource_ref(route);
        let manager = get_global_cross_ns_ref_manager();

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

#[async_trait::async_trait]
impl ProcessorHandler<TLSRoute> for TlsRouteHandler {
    fn validate(&self, route: &TLSRoute, _ctx: &HandlerContext) -> Vec<String> {
        let mut errors = validate_tls_route_if_enabled(route);
        let backend_ref_tuples = collect_backend_ref_tuples(route);
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        errors.extend(
            super::route_utils::validate_backend_refs(route_ns, &backend_ref_tuples)
                .iter()
                .map(|e| e.message()),
        );
        errors
    }

    async fn parse(&self, mut route: TLSRoute, _ctx: &HandlerContext) -> ProcessResult<TLSRoute> {
        // Record cross-namespace references for revalidation when ReferenceGrant changes
        Self::record_cross_ns_refs(&route);

        // Register Service backend references for cross-resource requeue
        Self::register_service_refs(&route);

        // Mark denied cross-namespace references
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

        // Treat resolved_ports as controller-derived state.
        // Clear any stale value before recomputing from current parentRefs.
        route.spec.resolved_ports = None;

        // Resolve listener ports from parentRefs → Gateway → listener.port
        //
        // Per Gateway API spec:
        //   - parentRef.port set → use that port directly
        //   - parentRef.sectionName set → find matching listener by name, use its port
        //   - neither set → attach to ALL listeners of the parent Gateway
        //
        // Only include ports from listeners whose allowedRoutes policy permits
        // this TLSRoute's namespace (default: Same).
        if let Some(parent_refs) = &route.spec.parent_refs {
            let mut ports = Vec::new();
            for parent_ref in parent_refs {
                if let Some(port) = parent_ref.port {
                    ports.push(port as u16);
                } else {
                    let gw_ns = parent_ref
                        .namespace
                        .as_deref()
                        .or(route.metadata.namespace.as_deref())
                        .unwrap_or("default");
                    if let Some(gateway) = super::route_utils::lookup_gateway(gw_ns, &parent_ref.name) {
                        if let Some(listeners) = &gateway.spec.listeners {
                            if let Some(section_name) = &parent_ref.section_name {
                                if let Some(listener) = listeners.iter().find(|l| l.name == *section_name) {
                                    if super::route_utils::listener_allows_route_namespace(
                                        &listener.allowed_routes,
                                        route_ns,
                                        gw_ns,
                                    ) {
                                        ports.push(listener.port as u16);
                                    }
                                }
                            } else {
                                for listener in listeners {
                                    if super::route_utils::listener_allows_route_namespace(
                                        &listener.allowed_routes,
                                        route_ns,
                                        gw_ns,
                                    ) {
                                        ports.push(listener.port as u16);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if !ports.is_empty() {
                ports.sort_unstable();
                ports.dedup();
                route.spec.resolved_ports = Some(ports);
            }
        }

        if route.spec.resolved_ports.is_none() {
            tracing::debug!(
                route = %route.metadata.name.as_deref().unwrap_or(""),
                ns = route_ns,
                "TLSRoute has no resolved_ports: Gateway not yet available or no matching listeners"
            );
        }

        ProcessResult::Continue(route)
    }

    async fn on_change(&self, route: &TLSRoute, ctx: &HandlerContext) {
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");

        update_gateway_route_index(
            ResourceKind::TLSRoute,
            route_ns,
            route_name,
            route.spec.parent_refs.as_ref(),
        );

        let tracker_changed = update_attached_route_tracker(
            ResourceKind::TLSRoute,
            route_ns,
            route_name,
            route.spec.parent_refs.as_ref(),
        );
        if tracker_changed {
            requeue_parent_gateways(route.spec.parent_refs.as_ref(), route_ns, ctx).await;
        }
    }

    async fn on_delete(&self, route: &TLSRoute, ctx: &HandlerContext) {
        let resource_ref = Self::create_resource_ref(route);
        get_global_cross_ns_ref_manager().clear_resource_refs(&resource_ref);

        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");
        super::route_utils::clear_service_backend_refs(ResourceKind::TLSRoute, route_ns, route_name);

        remove_from_gateway_route_index(ResourceKind::TLSRoute, route_ns, route_name);

        let tracker_changed = remove_from_attached_route_tracker(ResourceKind::TLSRoute, route_ns, route_name);
        if tracker_changed {
            requeue_parent_gateways(route.spec.parent_refs.as_ref(), route_ns, ctx).await;
        }
    }

    fn update_status(&self, route: &mut TLSRoute, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = route.metadata.generation;
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");

        let validation_accepted_errors = AcceptedError::from_validation_errors(validation_errors);

        let backend_ref_tuples = collect_backend_ref_tuples(route);
        let mut resolved_refs_errors = super::route_utils::validate_backend_refs(route_ns, &backend_ref_tuples);
        let ref_denied_list = collect_ref_denied_list(route);
        resolved_refs_errors.extend(super::route_utils::collect_ref_denied_errors(&ref_denied_list));

        let status = route.status.get_or_insert_with(|| TLSRouteStatus { parents: vec![] });
        let route_hostnames = route.spec.hostnames.as_ref();

        if let Some(parent_refs) = &route.spec.parent_refs {
            for parent_ref in parent_refs {
                let mut accepted_errors =
                    super::route_utils::validate_parent_ref_accepted(route_ns, parent_ref, route_hostnames);
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
    }
}

fn collect_backend_ref_tuples(route: &TLSRoute) -> Vec<(Option<&str>, Option<&str>, &str)> {
    let mut tuples = Vec::new();
    if let Some(rules) = &route.spec.rules {
        for rule in rules {
            if let Some(backend_refs) = &rule.backend_refs {
                for br in backend_refs {
                    tuples.push((br.kind.as_deref(), br.namespace.as_deref(), br.name.as_str()));
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
