//! Actual declaring HTTPRoute auth fields and non-authorizing effective locality.
use super::owned::{FieldContract, FieldSlot, FieldWire, OwnerSnapshot, PreparedSlot, PreparedTlsTarget, TopologyLoan};
use super::route_fields::{HttpRouteBackendRefs, RouteFilterSlot};
use super::template_fields::{PreparedTemplateAtom, TemplateAtom};
use super::{ObjectIdentity, ReferenceError, ReferenceToken, ResourceScope, TemplateSecretReference};
use crate::resources::common::external_http::TargetKind;
use crate::resources::common::OutboundTlsLocal;
use crate::resources::edgion_plugins::ForwardAuthConfig;
use crate::resources::http_route::{HTTPRouteFilter, HTTPRouteFilterType, HTTPRouteRule};
use crate::HTTPRoute;
use k8s_openapi::api::core::v1::Secret;
use serde::{Deserialize, Serialize};

/// Transport-only locality. This is neither an authored source nor Grant authority.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedExternalAuth {
    namespace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}
impl ResolvedExternalAuth {
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

fn filter_at(rules: &[HTTPRouteRule], slot: RouteFilterSlot) -> Option<&HTTPRouteFilter> {
    let rule = rules.get(slot.rule())?;
    let filters = match slot.backend() {
        None => rule.filters.as_ref()?,
        Some(index) => rule.backend_refs.as_ref()?.get(index)?.filters.as_ref()?,
    };
    filters.get(slot.filter())
}
fn config(filter: &HTTPRouteFilter) -> Result<&ForwardAuthConfig, ReferenceError> {
    if filter.filter_type != HTTPRouteFilterType::ExternalAuth {
        return Err(ReferenceError::UnsupportedTarget);
    }
    filter
        .external_auth
        .as_ref()
        .and_then(|auth| auth.edgion_config())
        .ok_or(ReferenceError::UnsupportedTarget)
}
fn validate_target(config: &ForwardAuthConfig, namespace: &str) -> Result<(), ReferenceError> {
    if let TargetKind::Service(service) = config
        .conn
        .target
        .kind()
        .map_err(|_| ReferenceError::UnsupportedTarget)?
    {
        super::targets::managed_http_service_target(service, namespace)?;
    }
    Ok(())
}
fn slots(rules: &[HTTPRouteRule]) -> Vec<RouteFilterSlot> {
    let mut result = Vec::new();
    for (rule, value) in rules.iter().enumerate() {
        for (filter, value) in value.filters.iter().flatten().enumerate() {
            if value.filter_type == HTTPRouteFilterType::ExternalAuth || value.external_auth.is_some() {
                result.push(RouteFilterSlot::new(rule, None, filter));
            }
        }
        for (backend, value) in value.backend_refs.iter().flatten().enumerate() {
            for (filter, value) in value.filters.iter().flatten().enumerate() {
                if value.filter_type == HTTPRouteFilterType::ExternalAuth || value.external_auth.is_some() {
                    result.push(RouteFilterSlot::new(rule, Some(backend), filter));
                }
            }
        }
    }
    result
}
fn tls_slot(slot: RouteFilterSlot, ca: Option<usize>) -> FieldSlot {
    FieldSlot::RouteAuthTls {
        rule: slot.rule(),
        backend: slot.backend(),
        filter: slot.filter(),
        ca,
    }
}
fn tls_slots(
    owner: &OwnerSnapshot,
    slot: RouteFilterSlot,
    config: &ForwardAuthConfig,
) -> (Vec<PreparedSlot>, Option<PreparedSlot>) {
    let Some(tls) = config.conn.tls.as_ref().filter(|tls| tls.enabled) else {
        return (Vec::new(), None);
    };
    let ca = tls
        .outbound
        .validation
        .as_ref()
        .and_then(|value| value.ca_certificate_refs.as_ref())
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(index, wire)| {
            PreparedSlot::new(
                owner,
                FieldWire::Ca(wire.clone()),
                FieldContract::CustomCa,
                tls_slot(slot, Some(index)),
            )
        })
        .collect();
    let client = tls.outbound.client_certificate_ref.as_ref().map(|wire| {
        PreparedSlot::new(
            owner,
            FieldWire::Secret(wire.clone()),
            FieldContract::CustomSecret,
            tls_slot(slot, None),
        )
    });
    (ca, client)
}

type RouteAuthTargetSnapshot = (
    bool,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i32>,
);

#[derive(PartialEq, Eq)]
pub(super) struct RouteAuthSnapshot {
    pub(super) owner: OwnerSnapshot,
    slot: RouteFilterSlot,
    target: RouteAuthTargetSnapshot,
    tls_shape: (bool, bool, bool, bool),
    ca: Vec<PreparedSlot>,
    client: Option<PreparedSlot>,
    templates: Vec<PreparedTemplateAtom>,
}
impl RouteAuthSnapshot {
    fn new(route: &HTTPRoute, slot: RouteFilterSlot) -> Result<Self, ReferenceError> {
        HttpRouteBackendRefs::from_route(route)?;
        let config = config(
            filter_at(route.spec.rules.as_deref().unwrap_or_default(), slot).ok_or(ReferenceError::StaleBinding)?,
        )?;
        validate_target(config, route.metadata.namespace.as_deref().unwrap_or("default"))?;
        let owner = OwnerSnapshot::namespaced("gateway.networking.k8s.io", "HTTPRoute", &route.metadata);
        let target = &config.conn.target;
        let target = (
            target.url.is_some(),
            target.service.group.clone(),
            target.service.kind.clone(),
            target.service.namespace.clone(),
            target.service.name.clone(),
            target.service.port,
        );
        let tls = config.conn.tls.as_ref();
        let tls_shape = (
            tls.is_some(),
            tls.is_some_and(|tls| tls.enabled),
            tls.is_some_and(|tls| tls.outbound.validation.is_some()),
            tls.and_then(|tls| tls.outbound.validation.as_ref())
                .is_some_and(|validation| validation.ca_certificate_refs.is_some()),
        );
        let (ca, client) = tls_slots(&owner, slot, config);
        let mut templates = Vec::new();
        super::template_fields::visit(&config.request, |atom: TemplateAtom| {
            templates.push(PreparedTemplateAtom::new(&owner, atom, |position, segment| {
                FieldSlot::RouteAuthTemplate {
                    rule: slot.rule(),
                    backend: slot.backend(),
                    filter: slot.filter(),
                    position,
                    segment,
                }
            }));
        });
        Ok(Self {
            owner,
            slot,
            target,
            tls_shape,
            ca,
            client,
            templates,
        })
    }
}

/// A reference-only snapshot of one actual authored rule/backend/filter position.
pub struct PreparedRouteAuthField {
    snapshot: RouteAuthSnapshot,
}
impl PreparedRouteAuthField {
    pub fn from_route(route: &HTTPRoute, slot: RouteFilterSlot) -> Result<Self, ReferenceError> {
        Ok(Self {
            snapshot: RouteAuthSnapshot::new(route, slot)?,
        })
    }
    pub fn bind<'a>(&'a self, route: &'a HTTPRoute) -> Result<AuthoredRouteAuthField<'a>, ReferenceError> {
        if self.snapshot != RouteAuthSnapshot::new(route, self.snapshot.slot)? {
            return Err(ReferenceError::StaleBinding);
        }
        let config = config(
            filter_at(route.spec.rules.as_deref().unwrap_or_default(), self.snapshot.slot)
                .ok_or(ReferenceError::StaleBinding)?,
        )?;
        Ok(AuthoredRouteAuthField {
            snapshot: &self.snapshot,
            route,
            config,
        })
    }
}

/// Only this actual authored loan can expose authorization tokens.
pub struct AuthoredRouteAuthField<'a> {
    snapshot: &'a RouteAuthSnapshot,
    route: &'a HTTPRoute,
    config: &'a ForwardAuthConfig,
}
impl AuthoredRouteAuthField<'_> {
    pub fn route(&self) -> &HTTPRoute {
        self.route
    }
    pub fn slot(&self) -> RouteFilterSlot {
        self.snapshot.slot
    }
    pub fn config(&self) -> &ForwardAuthConfig {
        self.config
    }
    pub fn namespace(&self) -> &str {
        self.route.metadata.namespace.as_deref().unwrap_or("default")
    }
    pub fn resolution(&self, error: Option<String>) -> ResolvedExternalAuth {
        ResolvedExternalAuth {
            namespace: self.namespace().to_string(),
            error,
        }
    }
    pub fn ca_certificates(&self) -> impl Iterator<Item = ReferenceToken<'_>> {
        self.snapshot
            .ca
            .iter()
            .map(|slot| slot.token(TopologyLoan::RouteAuth(self.snapshot)))
    }
    pub fn client_certificate(&self) -> Option<ReferenceToken<'_>> {
        self.snapshot
            .client
            .as_ref()
            .map(|slot| slot.token(TopologyLoan::RouteAuth(self.snapshot)))
    }
    pub fn request_secrets(&self) -> impl Iterator<Item = TemplateSecretReference<'_>> {
        self.snapshot
            .templates
            .iter()
            .filter_map(|atom| atom.reference(TopologyLoan::RouteAuth(self.snapshot)))
    }
}

/// A canonical route/filter facade; callers cannot create detached positions.
pub struct HttpRouteAuthRefs<'a> {
    route: &'a HTTPRoute,
}
impl<'a> HttpRouteAuthRefs<'a> {
    pub fn from_route(route: &'a HTTPRoute) -> Result<Self, ReferenceError> {
        HttpRouteBackendRefs::from_route(route)?;
        Ok(Self { route })
    }
    pub fn authored_slots(&self) -> Vec<RouteFilterSlot> {
        slots(self.route.spec.rules.as_deref().unwrap_or_default())
    }
    pub fn effective_at(
        &self,
        rule: usize,
        backend: Option<usize>,
        filter: usize,
    ) -> Result<EffectiveRouteAuthField<'a>, ReferenceError> {
        let slot = RouteFilterSlot::new(rule, backend, filter);
        let field = filter_at(
            self.route.effective_rules().map(Vec::as_slice).unwrap_or_default(),
            slot,
        )
        .ok_or(ReferenceError::StaleBinding)?;
        let config = config(field)?;
        let namespace = if self.route.spec.resolved_rules.is_some() {
            field
                .resolved_external_auth
                .as_ref()
                .ok_or(ReferenceError::StaleBinding)?
                .namespace
                .as_str()
        } else {
            self.route.metadata.namespace.as_deref().unwrap_or("default")
        };
        ObjectIdentity::checked(
            "gateway.networking.k8s.io",
            "HTTPRoute",
            ResourceScope::Namespaced(namespace),
            self.route.metadata.name.as_deref().unwrap_or_default(),
        )?;
        if self.route.spec.resolved_rules.is_none()
            && field
                .resolved_external_auth
                .as_ref()
                .is_some_and(|stamp| stamp.namespace != namespace)
        {
            return Err(ReferenceError::StaleBinding);
        }
        validate_target(config, namespace)?;
        let tls = config.conn.tls.as_ref().filter(|tls| tls.enabled);
        let ca = tls
            .and_then(|tls| tls.outbound.validation.as_ref())
            .and_then(|validation| validation.ca_certificate_refs.as_ref())
            .into_iter()
            .flatten()
            .map(|wire| PreparedTlsTarget::ca(namespace, wire))
            .collect();
        let client = tls
            .and_then(|tls| tls.outbound.client_certificate_ref.as_ref())
            .map(|wire| PreparedTlsTarget::client(namespace, wire));
        Ok(EffectiveRouteAuthField {
            route: self.route,
            field,
            config,
            namespace,
            ca,
            client,
        })
    }
}

/// Effective config remains borrowed through client preparation and construction.
/// Effective locality exposes no authored authorization capability.
/// ```compile_fail
/// use edgion_resources::resources::refs::route_auth_fields::EffectiveRouteAuthField;
/// fn no_grant(field: &EffectiveRouteAuthField<'_>) { field.grant_query(); }
/// ```
/// ```compile_fail
/// use edgion_resources::{HTTPRoute, resources::refs::route_auth_fields::HttpRouteAuthRefs};
/// fn cannot_drop(route: HTTPRoute) {
///     let refs = HttpRouteAuthRefs::from_route(&route).unwrap();
///     let field = refs.effective_at(0, None, 0).unwrap();
///     drop(route);
///     field.config();
/// }
/// ```
/// ```compile_fail
/// use edgion_resources::{HTTPRoute, resources::refs::route_auth_fields::HttpRouteAuthRefs};
/// fn cannot_mutate(mut route: HTTPRoute) {
///     let refs = HttpRouteAuthRefs::from_route(&route).unwrap();
///     let field = refs.effective_at(0, None, 0).unwrap();
///     route.metadata.namespace = None;
///     field.config();
/// }
/// ```
pub struct EffectiveRouteAuthField<'a> {
    route: &'a HTTPRoute,
    field: &'a HTTPRouteFilter,
    config: &'a ForwardAuthConfig,
    namespace: &'a str,
    ca: Vec<PreparedTlsTarget>,
    client: Option<PreparedTlsTarget>,
}
impl EffectiveRouteAuthField<'_> {
    pub fn config(&self) -> &ForwardAuthConfig {
        self.config
    }
    pub fn namespace(&self) -> &str {
        self.namespace
    }
    pub fn resolution_error(&self) -> Option<&str> {
        self.field
            .resolved_external_auth
            .as_ref()
            .and_then(ResolvedExternalAuth::error)
    }
    pub fn validate_source(&self) -> Result<(), ReferenceError> {
        HttpRouteBackendRefs::from_route(self.route).map(|_| ())
    }
    pub fn outbound_tls_selection(&self) -> Result<super::outbound_tls::BoundLocalTlsSelection<'_>, ReferenceError> {
        self.validate_source()?;
        if self.config.conn.tls.as_ref().is_some_and(|tls| tls.enabled) {
            Ok(super::outbound_tls::BoundLocalTlsSelection::from_active(
                super::outbound_tls::BoundOutboundTls::from_route_auth(RouteAuthTlsField { field: self }),
            ))
        } else {
            Ok(super::outbound_tls::BoundLocalTlsSelection::from_route_auth(self))
        }
    }
}

pub(super) struct RouteAuthTlsField<'a> {
    field: &'a EffectiveRouteAuthField<'a>,
}
impl RouteAuthTlsField<'_> {
    pub(super) fn validate_source(&self) -> Result<(), ReferenceError> {
        self.field.validate_source()
    }
    pub(super) fn policy(&self) -> &OutboundTlsLocal {
        &self
            .field
            .config
            .conn
            .tls
            .as_ref()
            .expect("active actual field")
            .outbound
    }
    pub(super) fn ca_material(&self) -> Option<&[Secret]> {
        self.field.config.conn.tls.as_ref()?.resolved_ca_certificates.as_deref()
    }
    pub(super) fn client_material(&self) -> Option<&Secret> {
        self.field
            .config
            .conn
            .tls
            .as_ref()?
            .resolved_client_certificate
            .as_ref()
    }
    pub(super) fn visit_ca<'a>(&'a self, mut visit: impl FnMut(&'a PreparedTlsTarget)) {
        for slot in &self.field.ca {
            visit(slot);
        }
    }
    pub(super) fn client_reference(&self) -> Option<&PreparedTlsTarget> {
        self.field.client.as_ref()
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/route_auth_fields_test.rs"]
mod tests;
