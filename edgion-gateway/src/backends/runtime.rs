//! Proof-carrying backend identities between route selection and transport.

use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

use crate::backends::discovery::ServiceSectionKey;
use crate::backends::policy::BackendTlsPolicyLookup;
use crate::lb::BackendSelector;
use crate::routes::http::match_unit::CompiledBackend;
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
use edgion_resources::resources::refs::route_fields::{
    EffectiveBackendReference, TcpRouteBackendRefs, TlsRouteBackendRefs, UdpRouteBackendRefs,
};
use edgion_resources::resources::refs::targets::BackendTarget;
use edgion_resources::resources::refs::ReferenceError;
use edgion_resources::{HTTPBackendRefKind, TCPRoute, TLSRoute, UDPRoute};

/// A canonical Service section compiled from an actual Route's effective field.
/// Its identity is a target projection, never ReferenceGrant authority.
/// ```compile_fail
/// use edgion_gateway::backends::CompiledServiceTarget;
/// fn overwrite(target: &mut CompiledServiceTarget) {
///     target.policy_target.name = "replacement".into();
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompiledServiceTarget {
    policy_target: PolicyTargetKey,
    key: String,
    section: ServiceSectionKey,
}

impl CompiledServiceTarget {
    pub fn policy_target(&self) -> &PolicyTargetKey {
        &self.policy_target
    }
    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn section(&self) -> &ServiceSectionKey {
        &self.section
    }
    pub fn namespace(&self) -> &str {
        &self.policy_target.namespace
    }
    pub fn name(&self) -> &str {
        &self.policy_target.name
    }
}

/// An immutable AI resource identity and its cold-compiled store lookup key.
/// The identity does not itself grant ReferenceGrant authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompiledAiTarget {
    policy_target: PolicyTargetKey,
    key: String,
}

impl CompiledAiTarget {
    pub fn policy_target(&self) -> &PolicyTargetKey {
        &self.policy_target
    }
    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn namespace(&self) -> &str {
        &self.policy_target.namespace
    }
    pub fn name(&self) -> &str {
        &self.policy_target.name
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompiledBackendTarget {
    Service(CompiledServiceTarget),
    EdgionBackend(CompiledAiTarget),
    Invalid { classified_kind: HTTPBackendRefKind },
}

impl CompiledBackendTarget {
    pub(crate) fn from_reference(
        reference: EffectiveBackendReference<'_>,
        classified_kind: HTTPBackendRefKind,
    ) -> Self {
        let (identity, target_kind) = match reference.target() {
            Ok(BackendTarget::Service(identity)) => (identity, PolicyTargetKind::Service),
            Ok(BackendTarget::EdgionBackend(identity)) => (identity, PolicyTargetKind::EdgionBackend),
            _ => return Self::Invalid { classified_kind },
        };
        let namespace = identity
            .scope()
            .namespace()
            .expect("native terminal targets are namespaced");
        let policy_target = PolicyTargetKey {
            target_kind,
            namespace: namespace.to_owned(),
            name: identity.name().to_owned(),
        };
        if target_kind == PolicyTargetKind::EdgionBackend {
            return Self::EdgionBackend(CompiledAiTarget {
                key: format!("{}/{}", policy_target.namespace, policy_target.name),
                policy_target,
            });
        }
        let section = crate::backends::service_section_key_from_port(reference.port())
            .expect("the canonical Service target validated its required numeric port");
        Self::Service(CompiledServiceTarget {
            key: format!("{namespace}/{}", identity.name()),
            policy_target,
            section,
        })
    }

    pub fn policy_target(&self) -> Option<&PolicyTargetKey> {
        match self {
            Self::Service(service) => Some(service.policy_target()),
            Self::EdgionBackend(target) => Some(target.policy_target()),
            Self::Invalid { .. } => None,
        }
    }
}

/// One immutable first-rule generation. Only actual L4 owners can construct it.
pub(crate) struct CompiledL4Backends {
    slots: Box<[CompiledL4Backend]>,
    selector: BackendSelector<usize>,
}

struct CompiledL4Backend {
    slot: usize,
    weight: Option<i32>,
    denied: bool,
    target: CompiledBackendTarget,
}

impl CompiledL4Backends {
    pub(crate) fn from_tcp(route: &TCPRoute) -> Result<Self, ReferenceError> {
        Ok(Self::from_references(
            TcpRouteBackendRefs::from_route(route)?.effective(),
        ))
    }

    pub(crate) fn from_tls(route: &TLSRoute) -> Result<Self, ReferenceError> {
        Ok(Self::from_references(
            TlsRouteBackendRefs::from_route(route)?.effective(),
        ))
    }

    pub(crate) fn from_udp(route: &UDPRoute) -> Result<Self, ReferenceError> {
        Ok(Self::from_references(
            UdpRouteBackendRefs::from_route(route)?.effective(),
        ))
    }

    fn from_references<'a>(references: impl Iterator<Item = EffectiveBackendReference<'a>>) -> Self {
        let slots: Box<[_]> = references
            .take_while(|reference| reference.slot().rule() == 0)
            .map(|reference| CompiledL4Backend {
                slot: reference.slot().backend(),
                weight: reference.weight(),
                denied: reference.ref_denied().is_some(),
                target: CompiledBackendTarget::from_reference(reference, HTTPBackendRefKind::Unsupported),
            })
            .collect();
        let selector = BackendSelector::new(
            (0..slots.len()).collect(),
            slots.iter().map(|slot| slot.weight).collect(),
        );
        Self { slots, selector }
    }

    #[inline]
    pub(crate) fn select(&self) -> Result<SelectedL4Backend<'_>, u32> {
        let index = self.selector.select()?;
        self.slots
            .get(index)
            .filter(|slot| slot.slot == index)
            .map(|slot| SelectedL4Backend { slot })
            .ok_or(crate::lb::backend_selector::ERR_NO_BACKEND_REFS)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum L4BackendRejection {
    Denied,
    Invalid,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum L4ResolveError<E> {
    Rejected(L4BackendRejection),
    Unavailable(E),
}

/// Borrowed selection from one sealed generation; it cannot be reconstructed
/// from a raw reference, target, index, or namespace by a transport caller.
#[derive(Clone, Copy)]
pub(crate) struct SelectedL4Backend<'a> {
    slot: &'a CompiledL4Backend,
}

impl<'a> SelectedL4Backend<'a> {
    #[inline]
    pub(crate) fn service(self) -> Result<&'a CompiledServiceTarget, L4BackendRejection> {
        if self.slot.denied {
            return Err(L4BackendRejection::Denied);
        }
        match &self.slot.target {
            CompiledBackendTarget::Service(target) => Ok(target),
            _ => Err(L4BackendRejection::Invalid),
        }
    }

    /// The closure is the actual discovery boundary in every L4 dial path.
    #[inline]
    pub(crate) fn with_service<T, E>(
        self,
        resolve: impl FnOnce(&'a CompiledServiceTarget) -> Result<T, E>,
    ) -> Result<(&'a CompiledServiceTarget, T), L4ResolveError<E>> {
        let target = self.service().map_err(L4ResolveError::Rejected)?;
        resolve(target)
            .map(|resolved| (target, resolved))
            .map_err(L4ResolveError::Unavailable)
    }
}

/// One immutable, gateway-compiled HTTPRoute backend slot.
#[derive(Clone)]
pub struct LogicalBackendHandle {
    generation: Arc<[CompiledBackend]>,
    index: usize,
}

impl LogicalBackendHandle {
    pub(crate) fn new(generation: Arc<[CompiledBackend]>, index: usize) -> Option<Self> {
        if generation.get(index).is_some_and(|compiled| compiled.slot() == index) {
            Some(Self { generation, index })
        } else {
            None
        }
    }

    #[inline]
    pub fn compiled(&self) -> &CompiledBackend {
        &self.generation[self.index]
    }

    #[inline]
    pub fn slot(&self) -> usize {
        self.compiled().slot()
    }

    #[inline]
    pub fn identity(&self) -> &edgion_resources::LogicalBackendSlotIdentity {
        self.compiled().identity()
    }

    #[inline]
    pub fn target(&self) -> &CompiledBackendTarget {
        self.compiled().target()
    }

    #[inline]
    pub fn declared_weight(&self) -> i32 {
        self.compiled().weight.unwrap_or(1)
    }

    #[inline]
    pub fn is_authorized(&self) -> bool {
        self.compiled().ref_denied.is_none()
    }

    /// Canonical resource identity used by backend-attached policy stores.
    ///
    /// Slot occurrence deliberately does not participate: two route slots that
    /// refer to the same backend resource share the same attached policy.
    pub fn policy_target_key(&self) -> Option<&PolicyTargetKey> {
        self.is_authorized().then(|| self.target().policy_target()).flatten()
    }

    pub fn service_target(&self) -> Option<&CompiledServiceTarget> {
        self.compiled().permitted_service()
    }
}

impl fmt::Debug for LogicalBackendHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LogicalBackendHandle")
            .field("slot", &self.slot())
            .field("identity", &self.identity())
            .field("target", &self.target())
            .field("declared_weight", &self.declared_weight())
            .field("authorized", &self.is_authorized())
            .finish()
    }
}

/// A Service slot after sticky/Internal/WRR logical selection and TLS-policy capture.
#[derive(Clone)]
pub struct SelectedServiceBackend {
    logical: LogicalBackendHandle,
    backend_tls_policy: BackendTlsPolicyLookup,
}

impl fmt::Debug for SelectedServiceBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SelectedServiceBackend")
            .field("logical", &self.logical)
            .field("backend_tls_policy", &self.backend_tls_policy.debug_label())
            .finish()
    }
}

impl SelectedServiceBackend {
    pub(crate) fn new(logical: LogicalBackendHandle, backend_tls_policy: BackendTlsPolicyLookup) -> Option<Self> {
        (logical.is_authorized() && matches!(logical.target(), CompiledBackendTarget::Service(_))).then_some(Self {
            logical,
            backend_tls_policy,
        })
    }

    #[inline]
    pub fn logical(&self) -> &LogicalBackendHandle {
        &self.logical
    }

    #[inline]
    pub fn backend_tls_policy(&self) -> &BackendTlsPolicyLookup {
        &self.backend_tls_policy
    }
}

#[derive(Clone, Debug)]
pub enum SelectedLogicalBackend {
    Service(SelectedServiceBackend),
    Ai(LogicalBackendHandle),
}

impl SelectedLogicalBackend {
    #[inline]
    pub fn logical(&self) -> &LogicalBackendHandle {
        match self {
            Self::Service(selected) => selected.logical(),
            Self::Ai(logical) => logical,
        }
    }

    #[inline]
    pub fn compiled(&self) -> &CompiledBackend {
        self.logical().compiled()
    }
}

/// The sole selected HTTP-backend fact stored in a request context.
pub enum SelectedHttpBackend {
    Service(SelectedServiceBackend),
    Ai(LogicalBackendHandle),
    Invalid(LogicalBackendHandle),
}

impl fmt::Debug for SelectedHttpBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Service(selected) => f.debug_tuple("Service").field(selected).finish(),
            Self::Ai(logical) => f.debug_tuple("Ai").field(logical).finish(),
            Self::Invalid(logical) => f
                .debug_struct("Invalid")
                .field("slot", &logical.slot())
                .field("identity", &logical.identity())
                .field("target", &logical.target())
                .finish(),
        }
    }
}

impl SelectedHttpBackend {
    #[inline]
    pub fn compiled(&self) -> &CompiledBackend {
        match self {
            Self::Service(selected) => selected.logical().compiled(),
            Self::Ai(logical) | Self::Invalid(logical) => logical.compiled(),
        }
    }

    #[inline]
    pub fn routed(&self) -> Option<SelectedLogicalBackend> {
        match self {
            Self::Service(selected) => Some(SelectedLogicalBackend::Service(selected.clone())),
            Self::Ai(logical) => Some(SelectedLogicalBackend::Ai(logical.clone())),
            Self::Invalid(_) => None,
        }
    }

    #[inline]
    pub fn is_ai(&self) -> bool {
        matches!(self, Self::Ai(_))
    }

    #[inline]
    pub fn plugin_runtime(&self) -> Arc<crate::plugins::PluginRuntime> {
        Arc::clone(&self.compiled().plugin_runtime)
    }
}

impl Deref for SelectedHttpBackend {
    type Target = CompiledBackend;

    fn deref(&self) -> &Self::Target {
        self.compiled()
    }
}

#[cfg(test)]
#[path = "../../tests/unit/backends/runtime_test.rs"]
mod tests;
