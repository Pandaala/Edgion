//! Exact cluster-owned outbound TLS references and administrative selections.
use super::owned::{FieldContract, FieldSlot, FieldWire, OwnerSnapshot, PreparedSlot, TopologyLoan};
use super::{ReferenceError, ReferenceToken};
use crate::resources::common::OutboundTlsConfig;
use crate::EdgionGatewayConfig;

pub struct PreparedEgcTlsReferences {
    snapshot: EgcTlsSnapshot,
}

pub(super) struct EgcTlsSnapshot {
    pub(super) owner: OwnerSnapshot,
    outbound: bool,
    validation: bool,
    ca_list: bool,
    ca: Vec<PreparedSlot>,
    client: Option<PreparedSlot>,
}

impl EgcTlsSnapshot {
    fn matches(&self, field: Option<&OutboundTlsConfig>) -> bool {
        if self.outbound != field.is_some() {
            return false;
        }
        let validation = field.and_then(|field| field.validation.as_ref());
        let ca = validation.and_then(|validation| validation.ca_certificate_refs.as_deref());
        if self.validation != validation.is_some() || self.ca_list != ca.is_some() {
            return false;
        }
        let ca = ca.unwrap_or_default();
        if self.ca.len() != ca.len()
            || !self
                .ca
                .iter()
                .zip(ca)
                .all(|(slot, wire)| matches!(&slot.wire, FieldWire::Ca(prepared) if prepared == wire))
        {
            return false;
        }
        match (
            &self.client,
            field.and_then(|field| field.client_certificate_ref.as_ref()),
        ) {
            (None, None) => true,
            (Some(slot), Some(wire)) => matches!(&slot.wire,FieldWire::Secret(prepared) if prepared == wire),
            _ => false,
        }
    }
}

impl PreparedEgcTlsReferences {
    pub fn from_config(config: &EdgionGatewayConfig) -> Self {
        let owner = OwnerSnapshot::egc(&config.metadata);
        let field = config.spec.outbound_tls.as_ref();
        let validation = field.and_then(|field| field.validation.as_ref());
        let ca = validation.and_then(|validation| validation.ca_certificate_refs.as_ref());
        let snapshot = EgcTlsSnapshot {
            outbound: field.is_some(),
            validation: validation.is_some(),
            ca_list: ca.is_some(),
            ca: ca
                .into_iter()
                .flatten()
                .enumerate()
                .map(|(index, wire)| {
                    PreparedSlot::new(
                        &owner,
                        FieldWire::Ca(wire.clone()),
                        FieldContract::CustomCa,
                        FieldSlot::EgcTls { ca: Some(index) },
                    )
                })
                .collect(),
            client: field
                .and_then(|field| field.client_certificate_ref.as_ref())
                .map(|wire| {
                    PreparedSlot::new(
                        &owner,
                        FieldWire::Secret(wire.clone()),
                        FieldContract::CustomSecret,
                        FieldSlot::EgcTls { ca: None },
                    )
                }),
            owner,
        };
        Self { snapshot }
    }
    fn check(&self, config: &EdgionGatewayConfig) -> Result<(), ReferenceError> {
        if !self.snapshot.owner.matches(&config.metadata) || !self.snapshot.matches(config.spec.outbound_tls.as_ref()) {
            return Err(ReferenceError::StaleBinding);
        }
        self.snapshot.owner.source()?;
        Ok(())
    }
    pub fn bind<'a>(&'a self, config: &'a EdgionGatewayConfig) -> Result<BoundEgcTlsConfig<'a>, ReferenceError> {
        self.check(config)?;
        Ok(BoundEgcTlsConfig {
            field: &config.spec.outbound_tls,
            snapshot: &self.snapshot,
        })
    }
    pub fn bind_mut<'a>(
        &'a self,
        config: &'a mut EdgionGatewayConfig,
    ) -> Result<BoundEgcTlsConfigMut<'a>, ReferenceError> {
        self.check(config)?;
        Ok(BoundEgcTlsConfigMut {
            field: &mut config.spec.outbound_tls,
            snapshot: &self.snapshot,
        })
    }
}

pub struct BoundEgcTlsConfig<'a> {
    field: &'a Option<OutboundTlsConfig>,
    snapshot: &'a EgcTlsSnapshot,
}
pub struct BoundEgcTlsConfigMut<'a> {
    field: &'a mut Option<OutboundTlsConfig>,
    snapshot: &'a EgcTlsSnapshot,
}

macro_rules! access {
    () => {
        pub fn references(&self) -> Result<EgcTlsReferences<'_>, ReferenceError> {
            if !self.snapshot.matches(self.field.as_ref()) {
                return Err(ReferenceError::StaleBinding);
            }
            Ok(EgcTlsReferences {
                snapshot: self.snapshot,
            })
        }
        pub fn outbound_tls(&self) -> Result<Option<super::outbound_tls::BoundGlobalOutboundTls<'_>>, ReferenceError> {
            let references = self.references()?;
            Ok(self.field.as_ref().map(|policy| {
                super::outbound_tls::BoundGlobalOutboundTls::from_egc(EgcTlsField { references, policy })
            }))
        }
    };
}
impl BoundEgcTlsConfig<'_> {
    access!();
}
impl BoundEgcTlsConfigMut<'_> {
    access!();
    pub fn outbound_tls_mut(&mut self) -> &mut Option<OutboundTlsConfig> {
        self.field
    }
}

#[derive(Clone, Copy)]
pub struct EgcTlsReferences<'a> {
    snapshot: &'a EgcTlsSnapshot,
}

/// Administrative authority limited to the actual EGC outbound CA/client fields.
/// Ordinary tokens cannot be upgraded into this capability.
/// ```compile_fail
/// use edgion_resources::resources::refs::egc_tls_fields::EgcTlsMaterialSelection;
/// let selection = EgcTlsMaterialSelection {};
/// ```
/// A live selection retains the actual EGC owner loan.
/// ```compile_fail
/// use edgion_resources::resources::refs::egc_tls_fields::PreparedEgcTlsReferences;
/// fn drop_owner(prepared: &PreparedEgcTlsReferences, owner: edgion_resources::EdgionGatewayConfig) {
///     let binding = prepared.bind(&owner).unwrap();
///     let references = binding.references().unwrap();
///     let selection = references.client_certificate().unwrap().unwrap();
///     drop(owner);
///     selection.reference().validated().unwrap();
/// }
/// ```
pub struct EgcTlsMaterialSelection<'a> {
    token: ReferenceToken<'a>,
    role: SelectionRole,
}
#[derive(Clone, Copy)]
enum SelectionRole {
    Ca,
    Client,
}
impl<'a> EgcTlsMaterialSelection<'a> {
    fn new(token: ReferenceToken<'a>, role: SelectionRole) -> Result<Self, ReferenceError> {
        token.validated()?;
        Ok(Self { token, role })
    }
    pub fn reference(&self) -> &ReferenceToken<'_> {
        &self.token
    }
    pub fn is_client_certificate(&self) -> bool {
        matches!(self.role, SelectionRole::Client)
    }
}

impl EgcTlsReferences<'_> {
    pub fn ca_certificates(&self) -> impl Iterator<Item = Result<EgcTlsMaterialSelection<'_>, ReferenceError>> {
        self.snapshot
            .ca
            .iter()
            .map(|slot| EgcTlsMaterialSelection::new(slot.token(TopologyLoan::Egc(self.snapshot)), SelectionRole::Ca))
    }
    pub fn client_certificate(&self) -> Option<Result<EgcTlsMaterialSelection<'_>, ReferenceError>> {
        self.snapshot.client.as_ref().map(|slot| {
            EgcTlsMaterialSelection::new(slot.token(TopologyLoan::Egc(self.snapshot)), SelectionRole::Client)
        })
    }
}

pub(super) struct EgcTlsField<'a> {
    pub(super) references: EgcTlsReferences<'a>,
    pub(super) policy: &'a OutboundTlsConfig,
}
impl EgcTlsField<'_> {
    pub(super) fn visit_ca<'loan>(&'loan self, mut visit: impl FnMut(ReferenceToken<'loan>)) {
        for slot in &self.references.snapshot.ca {
            visit(slot.token(TopologyLoan::Egc(self.references.snapshot)));
        }
    }
    pub(super) fn client_reference(&self) -> Option<ReferenceToken<'_>> {
        self.references
            .snapshot
            .client
            .as_ref()
            .map(|slot| slot.token(TopologyLoan::Egc(self.references.snapshot)))
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/egc_tls_fields_test.rs"]
mod tests;
