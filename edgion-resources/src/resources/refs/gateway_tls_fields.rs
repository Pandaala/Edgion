//! Native TLS references borrowed from their actual declaring Gateway fields.
use super::owned::{FieldContract, FieldSlot, FieldWire, OwnerSnapshot, PreparedSlot, TopologyLoan};
use super::ReferenceToken;
use crate::resources::gateway::FrontendTLSValidation;
use crate::Gateway;

/// Actual immutable owner; accessors prepare only the selected field, never a full vector.
pub struct GatewayTlsRefs<'a> {
    gateway: &'a Gateway,
    owner: OwnerSnapshot,
}

/// A synchronous actual-field loan. It cannot freeze or outlive its Gateway.
pub struct GatewayTlsField<'a> {
    _gateway: &'a Gateway,
    owner: &'a OwnerSnapshot,
    slot: PreparedSlot,
}

impl<'a> GatewayTlsRefs<'a> {
    pub fn from_gateway(gateway: &'a Gateway) -> Self {
        Self {
            gateway,
            owner: OwnerSnapshot::namespaced("gateway.networking.k8s.io", "Gateway", &gateway.metadata),
        }
    }
    fn field(&self, wire: FieldWire, contract: FieldContract, position: FieldSlot) -> GatewayTlsField<'_> {
        GatewayTlsField {
            _gateway: self.gateway,
            owner: &self.owner,
            slot: PreparedSlot::new(&self.owner, wire, contract, position),
        }
    }
    pub fn backend_client(&self) -> Option<GatewayTlsField<'_>> {
        let reference = self
            .gateway
            .spec
            .tls
            .as_ref()?
            .backend
            .as_ref()?
            .client_certificate_ref
            .as_ref()?;
        Some(self.field(
            FieldWire::Secret(reference.clone()),
            FieldContract::NativeSecret,
            FieldSlot::GatewayBackendTls,
        ))
    }
    pub fn serving(&self, listener: usize, reference: usize) -> Option<GatewayTlsField<'_>> {
        let wire = self
            .gateway
            .spec
            .listeners
            .as_ref()?
            .get(listener)?
            .tls
            .as_ref()?
            .certificate_refs
            .as_ref()?
            .get(reference)?;
        Some(self.field(
            FieldWire::Secret(wire.clone()),
            FieldContract::NativeSecret,
            FieldSlot::GatewayServing { listener, reference },
        ))
    }
    fn frontend_selection(&self, listener: usize) -> Option<(Option<usize>, &FrontendTLSValidation)> {
        let listener = self.gateway.spec.listeners.as_ref()?.get(listener)?;
        if !listener.protocol.eq_ignore_ascii_case("HTTPS") {
            return None;
        }
        let frontend = self.gateway.spec.tls.as_ref()?.frontend.as_ref()?;
        if frontend
            .per_port
            .as_ref()
            .is_some_and(|entries| entries.len() > crate::resources::gateway::MAX_GATEWAY_FRONTEND_PER_PORT)
        {
            return None;
        }
        match frontend
            .per_port
            .as_deref()
            .unwrap_or_default()
            .iter()
            .enumerate()
            .find(|(_, entry)| entry.port == listener.port)
        {
            Some((index, entry)) => entry
                .tls
                .validation
                .as_ref()
                .map(|validation| (Some(index), validation)),
            None => frontend
                .default
                .validation
                .as_ref()
                .map(|validation| (None, validation)),
        }
    }
    pub fn frontend_validation(&self, listener: usize) -> Option<&FrontendTLSValidation> {
        self.frontend_selection(listener).map(|(_, validation)| validation)
    }
    pub fn frontend_ca(&self, listener: usize, reference: usize) -> Option<GatewayTlsField<'_>> {
        let (per_port, validation) = self.frontend_selection(listener)?;
        let wire = validation.ca_certificate_refs.get(reference)?;
        Some(self.field(
            FieldWire::NativeCa(wire.clone()),
            FieldContract::NativeCa,
            FieldSlot::GatewayFrontendCa { per_port, reference },
        ))
    }
}
impl GatewayTlsField<'_> {
    /// Pair only the actual native backend client declaration with its carried Secret.
    /// Serving and frontend fields cannot use this backend-only material accessor.
    ///
    /// A matching owner's material remains borrowed through the actual field.
    /// ```
    /// use edgion_resources::{Gateway, resources::refs::gateway_tls_fields::GatewayTlsRefs};
    /// fn inspect(owner: Gateway) {
    ///     let fields = GatewayTlsRefs::from_gateway(&owner);
    ///     let field = fields.backend_client().unwrap();
    ///     let pair = field.backend_client_material().unwrap();
    ///     assert!(pair.material().is_some());
    /// }
    /// ```
    /// The owner cannot be dropped while its material loan remains in use.
    /// ```compile_fail,E0505
    /// use edgion_resources::{Gateway, resources::refs::gateway_tls_fields::GatewayTlsRefs};
    /// fn inspect(owner: Gateway) {
    ///     let fields = GatewayTlsRefs::from_gateway(&owner);
    ///     let field = fields.backend_client().unwrap();
    ///     let pair = field.backend_client_material().unwrap();
    ///     drop(owner);
    ///     assert!(pair.material().is_some());
    /// }
    /// ```
    /// The actual owner cannot be retargeted while its material loan is in use.
    /// ```compile_fail,E0506
    /// use edgion_resources::{Gateway, resources::refs::gateway_tls_fields::GatewayTlsRefs};
    /// fn inspect(mut owner: Gateway) {
    ///     let fields = GatewayTlsRefs::from_gateway(&owner);
    ///     let field = fields.backend_client().unwrap();
    ///     let pair = field.backend_client_material().unwrap();
    ///     owner.metadata.name = Some("replacement".into());
    ///     assert!(pair.material().is_some());
    /// }
    /// ```
    pub fn backend_client_material(
        &self,
    ) -> Result<super::outbound_tls::BoundTlsMaterial<'_>, super::outbound_tls::TlsMaterialIdentityError> {
        use super::outbound_tls::{BoundTlsMaterial, TlsMaterialIdentityError};
        if !matches!(self.slot.position(), FieldSlot::GatewayBackendTls) {
            return Err(TlsMaterialIdentityError::Reference(
                super::ReferenceError::UnsupportedTarget,
            ));
        }
        let material = self
            ._gateway
            .spec
            .tls
            .as_ref()
            .and_then(|tls| tls.backend.as_ref())
            .and_then(|backend| backend.resolved_client_certificate.as_ref());
        let pair = BoundTlsMaterial::from_gateway_backend(self, material);
        pair.validate_identity()?;
        Ok(pair)
    }

    pub fn token(&self) -> ReferenceToken<'_> {
        self.slot.token(TopologyLoan::GatewayTls(self.owner))
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/gateway_tls_fields_test.rs"]
mod tests;
