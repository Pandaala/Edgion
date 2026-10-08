//! Borrowed native Route fields. Only authored terminal fields carry Grant authority.
use super::extension_ref::{validate_extension_ref_filter, ExtensionRefValidationError};
use super::targets::{BackendTarget, NamespaceRoutesSelector};
use super::{GrantQuery, ObjectIdentity, ReferenceError, ReferenceSource, ResourceScope};
use crate::resources::common::RefDenied;
use crate::resources::grpc_route::{GRPCRouteFilter, GRPCRouteFilterType};
use crate::resources::http_route::{HTTPRouteFilter, HTTPRouteFilterType, GATEWAY_API_GROUP};
use crate::resources::{
    GRPCBackendRef, GRPCRoute, HTTPBackendRef, HTTPRoute, TCPBackendRef, TCPRoute, TLSBackendRef, TLSRoute,
    UDPBackendRef, UDPRoute,
};
use crate::ResourceKind;
use kube::api::ObjectMeta;

#[derive(Clone, Copy)]
enum RouteOwner<'a> {
    Http(&'a HTTPRoute),
    Grpc(&'a GRPCRoute),
    Tcp(&'a TCPRoute),
    Tls(&'a TLSRoute),
    Udp(&'a UDPRoute),
}
impl<'a> RouteOwner<'a> {
    fn metadata(self) -> &'a ObjectMeta {
        match self {
            Self::Http(route) => &route.metadata,
            Self::Grpc(route) => &route.metadata,
            Self::Tcp(route) => &route.metadata,
            Self::Tls(route) => &route.metadata,
            Self::Udp(route) => &route.metadata,
        }
    }
    fn kind(self) -> ResourceKind {
        match self {
            Self::Http(_) => ResourceKind::HTTPRoute,
            Self::Grpc(_) => ResourceKind::GRPCRoute,
            Self::Tcp(_) => ResourceKind::TCPRoute,
            Self::Tls(_) => ResourceKind::TLSRoute,
            Self::Udp(_) => ResourceKind::UDPRoute,
        }
    }
    fn namespace(self) -> &'a str {
        self.metadata().namespace.as_deref().unwrap_or("default")
    }
    fn identity(self) -> Result<ObjectIdentity<'a>, ReferenceError> {
        ObjectIdentity::checked(
            GATEWAY_API_GROUP,
            self.kind().as_str(),
            ResourceScope::Namespaced(self.namespace()),
            self.metadata().name.as_deref().unwrap_or_default(),
        )
    }
    fn source(self) -> Result<ReferenceSource<'a>, ReferenceError> {
        self.identity()?;
        ReferenceSource::checked(
            GATEWAY_API_GROUP,
            self.kind().as_str(),
            ResourceScope::Namespaced(self.namespace()),
        )
    }
}

#[derive(Clone, Copy)]
enum BackendField<'a> {
    Http(&'a HTTPBackendRef),
    Grpc(&'a GRPCBackendRef),
    Tcp(&'a TCPBackendRef),
    Tls(&'a TLSBackendRef),
    Udp(&'a UDPBackendRef),
}
macro_rules! field_value {
    ($self:ident, $field:ident) => {
        match $self {
            Self::Http(reference) => &reference.$field,
            Self::Grpc(reference) => &reference.$field,
            Self::Tcp(reference) => &reference.$field,
            Self::Tls(reference) => &reference.$field,
            Self::Udp(reference) => &reference.$field,
        }
    };
}
impl<'a> BackendField<'a> {
    fn target(self, namespace: &'a str) -> Result<BackendTarget<'a>, ReferenceError> {
        match self {
            Self::Http(reference) => reference.validated_target(namespace),
            Self::Grpc(reference) => reference.validated_target(namespace),
            Self::Tcp(reference) => reference.validated_target(namespace).map(BackendTarget::Service),
            Self::Tls(reference) => reference.validated_target(namespace).map(BackendTarget::Service),
            Self::Udp(reference) => reference.validated_target(namespace).map(BackendTarget::Service),
        }
    }
    fn name(self) -> &'a str {
        field_value!(self, name)
    }
    fn port(self) -> Option<i32> {
        *field_value!(self, port)
    }
    fn weight(self) -> Option<i32> {
        *field_value!(self, weight)
    }
    fn denied(self) -> Option<&'a RefDenied> {
        field_value!(self, ref_denied).as_ref()
    }
}

/// Position in the actual Route rule/backend vectors; constructors remain private.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteBackendSlot {
    rule: usize,
    backend: usize,
}
impl RouteBackendSlot {
    pub fn rule(self) -> usize {
        self.rule
    }
    pub fn backend(self) -> usize {
        self.backend
    }
}

/// An authored field borrowed from an actual native Route.
#[derive(Clone, Copy)]
pub struct AuthoredBackendReference<'a> {
    owner: RouteOwner<'a>,
    slot: RouteBackendSlot,
    field: BackendField<'a>,
}
impl<'a> AuthoredBackendReference<'a> {
    pub fn slot(self) -> RouteBackendSlot {
        self.slot
    }
    pub fn target_view(self) -> EffectiveBackendReference<'a> {
        EffectiveBackendReference {
            owner: self.owner,
            slot: self.slot,
            field: self.field,
        }
    }
    pub fn validated(self) -> Result<AuthoredRouteTarget<'a>, ReferenceError> {
        let source = self.owner.source()?;
        match self.field.target(self.owner.namespace())? {
            target @ (BackendTarget::Service(_) | BackendTarget::EdgionBackend(_)) => {
                Ok(AuthoredRouteTarget::Terminal(TerminalBackendReference {
                    field: self,
                    source,
                    target,
                }))
            }
            BackendTarget::Route(target) => Ok(AuthoredRouteTarget::NamedRoute(NamedRouteReference {
                _owner: self.owner,
                target,
            })),
            BackendTarget::NamespaceRoutes(selector) => {
                Ok(AuthoredRouteTarget::NamespaceRoutes(NamespaceRouteReference {
                    _owner: self.owner,
                    selector,
                }))
            }
        }
    }
}

pub enum AuthoredRouteTarget<'a> {
    Terminal(TerminalBackendReference<'a>),
    NamedRoute(NamedRouteReference<'a>),
    NamespaceRoutes(NamespaceRouteReference<'a>),
}

/// A valid authored terminal. A detached backend or effective field cannot mint one.
/// ```compile_fail
/// use edgion_resources::resources::refs::route_fields::TerminalBackendReference;
/// let terminal = TerminalBackendReference {};
/// ```
#[derive(Clone, Copy)]
pub struct TerminalBackendReference<'a> {
    field: AuthoredBackendReference<'a>,
    source: ReferenceSource<'a>,
    target: BackendTarget<'a>,
}
impl<'a> TerminalBackendReference<'a> {
    pub fn grant_query(self) -> GrantQuery<'a> {
        let (BackendTarget::Service(target) | BackendTarget::EdgionBackend(target)) = self.target else {
            unreachable!("terminal loans are constructed only for Service or EdgionBackend")
        };
        GrantQuery::new(self.source, target)
    }
    pub fn target(self) -> BackendTarget<'a> {
        self.target
    }
    pub fn source(self) -> ReferenceSource<'a> {
        self.source
    }
    pub fn source_kind(self) -> ResourceKind {
        self.field.owner.kind()
    }
    pub fn source_name(self) -> &'a str {
        self.field.owner.metadata().name.as_deref().unwrap_or_default()
    }
    pub fn source_uid(self) -> Option<&'a str> {
        self.field.owner.metadata().uid.as_deref()
    }
    pub fn source_generation(self) -> Option<i64> {
        self.field.owner.metadata().generation
    }
    pub fn slot(self) -> RouteBackendSlot {
        self.field.slot
    }
    pub fn port(self) -> Option<i32> {
        self.field.field.port()
    }
}

pub struct NamedRouteReference<'a> {
    _owner: RouteOwner<'a>,
    target: ObjectIdentity<'a>,
}
impl<'a> NamedRouteReference<'a> {
    pub fn target(&self) -> ObjectIdentity<'a> {
        self.target
    }
}
pub struct NamespaceRouteReference<'a> {
    _owner: RouteOwner<'a>,
    selector: NamespaceRoutesSelector<'a>,
}
impl<'a> NamespaceRouteReference<'a> {
    pub fn selector(&self) -> NamespaceRoutesSelector<'a> {
        self.selector
    }
}

/// A target-only view for status and AI facts; it is never authorization evidence.
/// ```compile_fail
/// use edgion_resources::resources::refs::route_fields::EffectiveBackendReference;
/// fn authorize(reference: EffectiveBackendReference<'_>) { reference.grant_query(); }
/// ```
#[derive(Clone, Copy)]
pub struct EffectiveBackendReference<'a> {
    owner: RouteOwner<'a>,
    slot: RouteBackendSlot,
    field: BackendField<'a>,
}
impl<'a> EffectiveBackendReference<'a> {
    pub fn target(self) -> Result<BackendTarget<'a>, ReferenceError> {
        self.owner.source()?;
        self.field.target(self.owner.namespace())
    }
    pub fn slot(self) -> RouteBackendSlot {
        self.slot
    }
    pub fn name(self) -> &'a str {
        self.field.name()
    }
    pub fn port(self) -> Option<i32> {
        self.field.port()
    }
    pub fn weight(self) -> Option<i32> {
        self.field.weight()
    }
    pub fn ref_denied(self) -> Option<&'a RefDenied> {
        self.field.denied()
    }
}

macro_rules! route_facade {
    ($facade:ident, $route:ty, $variant:ident, $effective:expr) => {
        pub struct $facade<'a> {
            route: &'a $route,
        }
        impl<'a> $facade<'a> {
            pub fn from_route(route: &'a $route) -> Result<Self, ReferenceError> {
                RouteOwner::$variant(route).source()?;
                Ok(Self { route })
            }
            pub fn authored(&self) -> impl Iterator<Item = AuthoredBackendReference<'a>> + 'a {
                let owner = RouteOwner::$variant(self.route);
                self.route
                    .spec
                    .rules
                    .iter()
                    .flatten()
                    .enumerate()
                    .flat_map(move |(rule, value)| {
                        value
                            .backend_refs
                            .iter()
                            .flatten()
                            .enumerate()
                            .map(move |(backend, field)| AuthoredBackendReference {
                                owner,
                                slot: RouteBackendSlot { rule, backend },
                                field: BackendField::$variant(field),
                            })
                    })
            }
            pub fn authored_at(&self, rule: usize, backend: usize) -> Option<AuthoredBackendReference<'a>> {
                let field = self
                    .route
                    .spec
                    .rules
                    .as_ref()?
                    .get(rule)?
                    .backend_refs
                    .as_ref()?
                    .get(backend)?;
                Some(AuthoredBackendReference {
                    owner: RouteOwner::$variant(self.route),
                    slot: RouteBackendSlot { rule, backend },
                    field: BackendField::$variant(field),
                })
            }
            pub fn effective_at(&self, rule: usize, backend: usize) -> Option<EffectiveBackendReference<'a>> {
                let field = ($effective)(self.route)?
                    .get(rule)?
                    .backend_refs
                    .as_ref()?
                    .get(backend)?;
                Some(EffectiveBackendReference {
                    owner: RouteOwner::$variant(self.route),
                    slot: RouteBackendSlot { rule, backend },
                    field: BackendField::$variant(field),
                })
            }
            pub fn effective(&self) -> impl Iterator<Item = EffectiveBackendReference<'a>> + 'a {
                let owner = RouteOwner::$variant(self.route);
                ($effective)(self.route)
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .flat_map(move |(rule, value)| {
                        value
                            .backend_refs
                            .iter()
                            .flatten()
                            .enumerate()
                            .map(move |(backend, field)| EffectiveBackendReference {
                                owner,
                                slot: RouteBackendSlot { rule, backend },
                                field: BackendField::$variant(field),
                            })
                    })
            }
        }
    };
}
route_facade!(HttpRouteBackendRefs, HTTPRoute, Http, HTTPRoute::effective_rules);
route_facade!(GrpcRouteBackendRefs, GRPCRoute, Grpc, GRPCRoute::effective_rules);
route_facade!(TcpRouteBackendRefs, TCPRoute, Tcp, |route: &'a TCPRoute| route
    .spec
    .rules
    .as_ref());
route_facade!(TlsRouteBackendRefs, TLSRoute, Tls, |route: &'a TLSRoute| route
    .spec
    .rules
    .as_ref());
route_facade!(UdpRouteBackendRefs, UDPRoute, Udp, |route: &'a UDPRoute| route
    .spec
    .rules
    .as_ref());

/// Position of a filter in the actual Route vectors. No detached slot constructor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteFilterSlot {
    rule: usize,
    backend: Option<usize>,
    filter: usize,
}
impl RouteFilterSlot {
    pub fn rule(self) -> usize {
        self.rule
    }
    pub fn backend(self) -> Option<usize> {
        self.backend
    }
    pub fn filter(self) -> usize {
        self.filter
    }
}

#[derive(Clone, Copy)]
enum ExtensionField<'a> {
    Http(&'a HTTPRouteFilter),
    Grpc(&'a GRPCRouteFilter),
}

/// A filter loan always borrows an actual Route and its actual filter position.
/// Effective loans describe the published root and existing child-local namespace
/// stamp; the transport does not carry the original child's UID or generation.
/// ```compile_fail
/// use edgion_resources::resources::refs::route_fields::RouteExtensionField;
/// let field = RouteExtensionField {};
/// ```
#[derive(Clone, Copy)]
pub struct RouteExtensionField<'a> {
    owner: RouteOwner<'a>,
    slot: RouteFilterSlot,
    field: ExtensionField<'a>,
    resolved: bool,
}
impl<'a> RouteExtensionField<'a> {
    pub fn slot(self) -> RouteFilterSlot {
        self.slot
    }
    pub fn validated(self) -> Result<Option<RouteExtensionLookup<'a>>, ExtensionRefValidationError> {
        let reference = match self.field {
            ExtensionField::Http(filter) => validate_extension_ref_filter(
                filter.filter_type == HTTPRouteFilterType::ExtensionRef,
                filter.extension_ref.as_ref(),
            ),
            ExtensionField::Grpc(filter) => validate_extension_ref_filter(
                filter.filter_type == GRPCRouteFilterType::ExtensionRef,
                filter.extension_ref.as_ref(),
            ),
        }?;
        let Some(reference) = reference else { return Ok(None) };
        let namespace = if self.resolved {
            reference
                .resolved_namespace
                .as_deref()
                .unwrap_or(self.owner.namespace())
        } else {
            self.owner.namespace()
        };
        let target = ObjectIdentity::checked(
            &reference.group,
            &reference.kind,
            ResourceScope::Namespaced(namespace),
            &reference.name,
        )
        .map_err(|_| ExtensionRefValidationError::UnsupportedTarget)?;
        Ok(Some(RouteExtensionLookup { field: self, target }))
    }
}

/// An owner-bound local plugin lookup; neither authored nor effective views grant
/// cross-namespace authority.
/// ```compile_fail
/// use edgion_resources::resources::refs::route_fields::RouteExtensionLookup;
/// fn authorize(reference: RouteExtensionLookup<'_>) { reference.grant_query(); }
/// ```
#[derive(Clone, Copy)]
pub struct RouteExtensionLookup<'a> {
    field: RouteExtensionField<'a>,
    target: ObjectIdentity<'a>,
}
impl<'a> RouteExtensionLookup<'a> {
    pub fn target(self) -> ObjectIdentity<'a> {
        self.target
    }
    pub fn source(self) -> ObjectIdentity<'a> {
        self.field
            .owner
            .identity()
            .expect("the owning Route facade validated this immutably borrowed source")
    }
    pub fn source_kind(self) -> ResourceKind {
        self.field.owner.kind()
    }
    pub fn source_uid(self) -> Option<&'a str> {
        self.field.owner.metadata().uid.as_deref()
    }
    pub fn source_generation(self) -> Option<i64> {
        self.field.owner.metadata().generation
    }
    pub fn slot(self) -> RouteFilterSlot {
        self.field.slot
    }
}

macro_rules! extension_facade {
    ($facade:ident, $route:ty, $variant:ident) => {
        pub struct $facade<'a> {
            route: &'a $route,
        }
        impl<'a> $facade<'a> {
            pub fn from_route(route: &'a $route) -> Result<Self, ReferenceError> {
                RouteOwner::$variant(route).identity()?;
                Ok(Self { route })
            }
            pub fn authored(&self) -> impl Iterator<Item = RouteExtensionField<'a>> + 'a {
                Self::fields(self.route, false)
            }
            pub fn effective(&self) -> impl Iterator<Item = RouteExtensionField<'a>> + 'a {
                Self::fields(self.route, self.route.spec.resolved_rules.is_some())
            }
            pub fn authored_at(
                &self,
                rule: usize,
                backend: Option<usize>,
                filter: usize,
            ) -> Option<RouteExtensionField<'a>> {
                self.at(rule, backend, filter, false)
            }
            pub fn effective_at(
                &self,
                rule: usize,
                backend: Option<usize>,
                filter: usize,
            ) -> Option<RouteExtensionField<'a>> {
                self.at(rule, backend, filter, self.route.spec.resolved_rules.is_some())
            }
            fn at(
                &self,
                rule: usize,
                backend: Option<usize>,
                filter: usize,
                resolved: bool,
            ) -> Option<RouteExtensionField<'a>> {
                let rules = if resolved {
                    self.route.spec.resolved_rules.as_ref()
                } else {
                    self.route.spec.rules.as_ref()
                };
                let value = rules?.get(rule)?;
                let filters = match backend {
                    Some(index) => value.backend_refs.as_ref()?.get(index)?.filters.as_ref(),
                    None => value.filters.as_ref(),
                }?;
                Some(RouteExtensionField {
                    owner: RouteOwner::$variant(self.route),
                    slot: RouteFilterSlot {
                        rule,
                        backend,
                        filter,
                    },
                    field: ExtensionField::$variant(filters.get(filter)?),
                    resolved,
                })
            }
            fn fields(route: &'a $route, resolved: bool) -> impl Iterator<Item = RouteExtensionField<'a>> + 'a {
                let rules = if resolved {
                    route.spec.resolved_rules.as_ref()
                } else {
                    route.spec.rules.as_ref()
                };
                rules
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .flat_map(move |(rule, value)| {
                        value
                            .filters
                            .iter()
                            .flatten()
                            .enumerate()
                            .map(|(filter, field)| (None, filter, field))
                            .chain(
                                value
                                    .backend_refs
                                    .iter()
                                    .flatten()
                                    .enumerate()
                                    .flat_map(|(backend, value)| {
                                        value
                                            .filters
                                            .iter()
                                            .flatten()
                                            .enumerate()
                                            .map(move |(filter, field)| (Some(backend), filter, field))
                                    }),
                            )
                            .map(move |(backend, filter, field)| RouteExtensionField {
                                owner: RouteOwner::$variant(route),
                                slot: RouteFilterSlot {
                                    rule,
                                    backend,
                                    filter,
                                },
                                field: ExtensionField::$variant(field),
                                resolved,
                            })
                    })
            }
        }
    };
}
extension_facade!(HttpRouteExtensionRefs, HTTPRoute, Http);
extension_facade!(GrpcRouteExtensionRefs, GRPCRoute, Grpc);

/// A live terminal retains its actual owning Route borrow.
/// ```compile_fail
/// use edgion_resources::{HTTPRoute, resources::refs::route_fields::{HttpRouteBackendRefs, AuthoredRouteTarget}};
/// fn mutate(route: &mut HTTPRoute) {
///     let refs = HttpRouteBackendRefs::from_route(route).unwrap();
///     let AuthoredRouteTarget::Terminal(terminal) = refs.authored().next().unwrap().validated().unwrap() else { return };
///     route.metadata.name = Some("replacement".into());
///     terminal.grant_query();
/// }
/// ```
const _: () = ();

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/route_fields_test.rs"]
mod tests;
