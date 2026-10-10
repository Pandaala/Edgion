//! Provider-owned LinkSys auth, bootstrap and enabled TLS reference loans.

use super::owned::{topology_revision, TopologyLoan};
use serde::Serialize;
use std::mem::{discriminant, Discriminant};
use std::sync::Arc;

use super::owned::{FieldContract, FieldSlot, FieldWire, OwnerSnapshot, PreparedSlot};
use super::template_fields::{PreparedTemplateAtom, TemplateAtom};
use super::{ReferenceError, ReferenceToken};
use crate::resources::common::{CustomCACertificateRef, SecretObjectReference, TlsConfig};
use crate::resources::link_sys::elasticsearch::ElasticsearchAuth;
use crate::resources::link_sys::{CredentialSourceProvider, LinkSys, SystemConfig};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(super) enum Provider {
    Redis,
    Etcd,
    Elasticsearch,
    Kafka,
    Otlp,
    Webhook,
    HttpDns,
    OAuth2,
    Unsupported,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(super) enum Role {
    Auth,
    Active,
    Previous,
    Connection,
    Tls,
    Enabled,
    Validation,
    Ca,
    Client,
}

enum Atom<'a> {
    Template(TemplateAtom),
    Presence(Role, bool),
    Variant(&'static str),
    Secret(Role, &'a SecretObjectReference),
    Ca(usize, &'a CustomCACertificateRef),
}
#[derive(PartialEq, Eq, Serialize)]
enum PreparedAtom {
    Template(PreparedTemplateAtom),
    Presence(Role, bool),
    Variant(&'static str),
    Reference {
        role: Role,
        ordinal: usize,
        slot: Box<PreparedSlot>,
    },
}

fn provider(config: &SystemConfig) -> Provider {
    match config {
        SystemConfig::Redis(_) => Provider::Redis,
        SystemConfig::Etcd(_) => Provider::Etcd,
        SystemConfig::Elasticsearch(_) => Provider::Elasticsearch,
        SystemConfig::Kafka(_) => Provider::Kafka,
        SystemConfig::Otlp(_) => Provider::Otlp,
        SystemConfig::Webhook(_) => Provider::Webhook,
        SystemConfig::HttpDns(_) => Provider::HttpDns,
        SystemConfig::CredentialSource(_) => Provider::OAuth2,
        SystemConfig::Unsupported => Provider::Unsupported,
    }
}

fn secret<'a>(role: Role, wire: Option<&'a SecretObjectReference>, visitor: &mut impl FnMut(Atom<'a>)) {
    visitor(Atom::Presence(role, wire.is_some()));
    if let Some(wire) = wire {
        visitor(Atom::Secret(role, wire));
    }
}
fn tls<'a>(wire: Option<&'a TlsConfig>, visitor: &mut impl FnMut(Atom<'a>)) {
    visitor(Atom::Presence(Role::Tls, wire.is_some()));
    if let Some(wire) = wire {
        visitor(Atom::Presence(Role::Enabled, wire.enabled));
        visitor(Atom::Presence(Role::Validation, wire.outbound.validation.is_some()));
        let cas = wire
            .outbound
            .validation
            .as_ref()
            .and_then(|validation| validation.ca_certificate_refs.as_ref());
        visitor(Atom::Presence(Role::Ca, cas.is_some()));
        for (ordinal, wire) in cas.into_iter().flatten().enumerate() {
            visitor(Atom::Ca(ordinal, wire));
        }
        secret(Role::Client, wire.outbound.client_certificate_ref.as_ref(), visitor);
    }
}

// One borrowed provider traversal is used for preparation and every re-loan.
// It never copies submitted or Controller-resolved material.
fn visit<'a>(config: &'a SystemConfig, visitor: &mut impl FnMut(Atom<'a>)) {
    match config {
        SystemConfig::Redis(config) => {
            secret(
                Role::Auth,
                config.auth.as_ref().map(|auth| &auth.slot.secret_ref),
                visitor,
            );
            tls(config.tls.as_ref(), visitor);
        }
        SystemConfig::Etcd(config) => {
            secret(
                Role::Auth,
                config.auth.as_ref().map(|auth| &auth.slot.secret_ref),
                visitor,
            );
            tls(config.tls.as_ref(), visitor);
        }
        SystemConfig::Elasticsearch(config) => {
            let wire = config.auth.as_ref().map(|auth| match auth {
                ElasticsearchAuth::Basic { slot } => {
                    visitor(Atom::Variant("basic"));
                    &slot.secret_ref
                }
                ElasticsearchAuth::ApiKey { slot } => {
                    visitor(Atom::Variant("apiKey"));
                    &slot.secret_ref
                }
                ElasticsearchAuth::Bearer { slot } => {
                    visitor(Atom::Variant("bearer"));
                    &slot.secret_ref
                }
            });
            secret(Role::Auth, wire, visitor);
            tls(config.tls.as_ref(), visitor);
        }
        SystemConfig::Kafka(config) => {
            secret(
                Role::Auth,
                config.sasl.as_ref().map(|sasl| &sasl.password.secret_ref),
                visitor,
            );
            tls(config.tls.as_ref(), visitor);
        }
        SystemConfig::Otlp(config) => {
            secret(
                Role::Auth,
                config.auth.as_ref().map(|auth| &auth.slot.secret_ref),
                visitor,
            );
            tls(config.tls.as_ref(), visitor);
        }
        SystemConfig::Webhook(config) => {
            tls(config.conn.tls.as_ref(), visitor);
            super::template_fields::visit(&config.request, |atom| visitor(Atom::Template(atom)));
        }
        SystemConfig::HttpDns(config) => {
            visitor(Atom::Presence(Role::Connection, config.connection.is_some()));
            tls(
                config
                    .connection
                    .as_ref()
                    .and_then(|connection| connection.tls.as_ref()),
                visitor,
            );
        }
        SystemConfig::CredentialSource(config) => match &config.provider {
            CredentialSourceProvider::OAuth2ClientCredentials(provider) => {
                secret(
                    Role::Active,
                    Some(&provider.client_authentication.active_secret_ref),
                    visitor,
                );
                secret(
                    Role::Previous,
                    provider.client_authentication.previous_secret_ref.as_ref(),
                    visitor,
                );
                tls(provider.tls.as_ref(), visitor);
            }
        },
        SystemConfig::Unsupported => {}
    }
}

#[derive(PartialEq, Eq, Serialize)]
struct PreparedConfig {
    #[serde(skip)]
    variant: Discriminant<SystemConfig>,
    provider: Provider,
    atoms: Vec<PreparedAtom>,
}
impl PreparedConfig {
    fn new(owner: &OwnerSnapshot, config: &SystemConfig) -> Self {
        let provider = provider(config);
        let mut atoms = Vec::new();
        visit(config, &mut |atom| {
            atoms.push(match atom {
                Atom::Template(atom) => {
                    PreparedAtom::Template(PreparedTemplateAtom::new(owner, atom, |position, segment| {
                        FieldSlot::LinkSysTemplate {
                            provider,
                            position,
                            segment,
                        }
                    }))
                }
                Atom::Presence(role, value) => PreparedAtom::Presence(role, value),
                Atom::Variant(variant) => PreparedAtom::Variant(variant),
                Atom::Secret(role, wire) => PreparedAtom::Reference {
                    role,
                    ordinal: 0,
                    slot: Box::new(PreparedSlot::new(
                        owner,
                        FieldWire::Secret(wire.clone()),
                        FieldContract::CustomSecret,
                        FieldSlot::LinkSys {
                            provider,
                            role,
                            reference: 0,
                        },
                    )),
                },
                Atom::Ca(ordinal, wire) => PreparedAtom::Reference {
                    role: Role::Ca,
                    ordinal,
                    slot: Box::new(PreparedSlot::new(
                        owner,
                        FieldWire::Ca(wire.clone()),
                        FieldContract::CustomSecret,
                        FieldSlot::LinkSys {
                            provider,
                            role: Role::Ca,
                            reference: ordinal,
                        },
                    )),
                },
            })
        });
        Self {
            variant: discriminant(config),
            provider,
            atoms,
        }
    }
    fn matches(&self, config: &SystemConfig) -> bool {
        if self.variant != discriminant(config) {
            return false;
        }
        let mut expected = self.atoms.iter();
        let mut matches = true;
        visit(config, &mut |atom| {
            matches &= match (expected.next(), atom) {
                (Some(PreparedAtom::Template(prepared)), Atom::Template(current)) => prepared.matches(&current),
                (Some(PreparedAtom::Presence(role, value)), Atom::Presence(current_role, current_value)) => {
                    *role == current_role && *value == current_value
                }
                (Some(PreparedAtom::Variant(variant)), Atom::Variant(current)) => *variant == current,
                (Some(PreparedAtom::Reference { role, ordinal: 0, slot }), Atom::Secret(current_role, wire)) => {
                    *role == current_role
                        && slot.matches_link_sys(self.provider, *role, 0)
                        && matches!(&slot.wire, FieldWire::Secret(prepared) if prepared == wire)
                }
                (
                    Some(PreparedAtom::Reference {
                        role: Role::Ca,
                        ordinal,
                        slot,
                    }),
                    Atom::Ca(current_ordinal, wire),
                ) => {
                    *ordinal == current_ordinal
                        && slot.matches_link_sys(self.provider, Role::Ca, *ordinal)
                        && matches!(&slot.wire, FieldWire::Ca(prepared) if prepared == wire)
                }
                _ => false,
            };
        });
        matches && expected.next().is_none()
    }
}

/// A cold reference-only snapshot from a concrete LinkSys owner.
pub struct PreparedLinkSysReferences {
    snapshot: Arc<LinkSysSnapshot>,
}
#[derive(PartialEq, Eq, Serialize)]
pub(super) struct LinkSysSnapshot {
    pub(super) owner: OwnerSnapshot,
    config: PreparedConfig,
    #[serde(skip)]
    pub(super) revision: String,
}
impl LinkSysSnapshot {
    pub(super) fn matches(&self, owner: &LinkSys) -> bool {
        self.owner.matches(&owner.metadata) && self.config.matches(&owner.spec.config)
    }
    pub(super) fn slot(&self, position: &FieldSlot) -> Option<&PreparedSlot> {
        self.config.atoms.iter().find_map(|atom| match atom {
            PreparedAtom::Reference { slot, .. } if slot.position() == position => Some(slot.as_ref()),
            PreparedAtom::Template(atom) => atom.slot().filter(|slot| slot.position() == position),
            _ => None,
        })
    }
}
impl PreparedLinkSysReferences {
    pub fn from_link_sys(owner: &LinkSys) -> Self {
        let source = OwnerSnapshot::namespaced("edgion.io", "LinkSys", &owner.metadata);
        let mut snapshot = LinkSysSnapshot {
            config: PreparedConfig::new(&source, &owner.spec.config),
            owner: source,
            revision: String::new(),
        };
        snapshot.revision = topology_revision(&snapshot);
        Self {
            snapshot: Arc::new(snapshot),
        }
    }
    /// Borrow this owner's active non-TLS auth material after exact field validation.
    /// Missing material stays pending; no other provider or TLS slot supplies auth.
    /// The returned borrow belongs to the actual owner, not this prepared snapshot.
    ///
    /// ```compile_fail
    /// use edgion_resources::resources::link_sys::LinkSys;
    /// use edgion_resources::resources::refs::link_sys_fields::PreparedLinkSysReferences;
    /// fn cannot_drop_owner(owner: LinkSys) {
    ///     let prepared = PreparedLinkSysReferences::from_link_sys(&owner);
    ///     let secret = prepared.resolved_auth_secret(&owner).unwrap();
    ///     drop(owner);
    ///     let _ = secret.is_some();
    /// }
    /// ```
    /// ```compile_fail
    /// use edgion_resources::resources::link_sys::LinkSys;
    /// use edgion_resources::resources::refs::link_sys_fields::PreparedLinkSysReferences;
    /// fn cannot_replace_owner(mut owner: LinkSys) {
    ///     let prepared = PreparedLinkSysReferences::from_link_sys(&owner);
    ///     let secret = prepared.resolved_auth_secret(&owner).unwrap();
    ///     owner.metadata.name = None;
    ///     let _ = secret.is_some();
    /// }
    /// ```
    pub fn resolved_auth_secret<'owner>(
        &self,
        owner: &'owner LinkSys,
    ) -> Result<Option<&'owner k8s_openapi::api::core::v1::Secret>, ReferenceError> {
        let bound = self.bind(owner)?;
        let secret = match &owner.spec.config {
            SystemConfig::Redis(config) => config.auth.as_ref().and_then(|auth| auth.slot.secret.as_ref()),
            SystemConfig::Etcd(config) => config.auth.as_ref().and_then(|auth| auth.slot.secret.as_ref()),
            SystemConfig::Elasticsearch(config) => config.auth.as_ref().and_then(|auth| match auth {
                ElasticsearchAuth::Basic { slot }
                | ElasticsearchAuth::ApiKey { slot }
                | ElasticsearchAuth::Bearer { slot } => slot.secret.as_ref(),
            }),
            SystemConfig::Kafka(config) => config.sasl.as_ref().and_then(|auth| auth.password.secret.as_ref()),
            SystemConfig::Otlp(config) => config.auth.as_ref().and_then(|auth| auth.slot.secret.as_ref()),
            SystemConfig::Webhook(_)
            | SystemConfig::HttpDns(_)
            | SystemConfig::CredentialSource(_)
            | SystemConfig::Unsupported => None,
        };
        let Some(secret) = secret else {
            return Ok(None);
        };
        let references = bound.references()?;
        let token = references.auth_secret().ok_or(ReferenceError::StaleBinding)?;
        let reference = token.validated()?;
        super::owned::validate_secret_metadata(reference, secret)?;
        Ok(Some(secret))
    }
    fn matches(&self, owner: &LinkSys) -> bool {
        self.snapshot.matches(owner)
    }
    pub fn bind<'a>(&'a self, owner: &'a LinkSys) -> Result<BoundLinkSysConfig<'a>, ReferenceError> {
        if !self.matches(owner) {
            return Err(ReferenceError::StaleBinding);
        }
        Ok(BoundLinkSysConfig {
            config: &owner.spec.config,
            prepared: self,
        })
    }
    pub fn bind_mut<'a>(&'a self, owner: &'a mut LinkSys) -> Result<BoundLinkSysConfigMut<'a>, ReferenceError> {
        if !self.matches(owner) {
            return Err(ReferenceError::StaleBinding);
        }
        Ok(BoundLinkSysConfigMut {
            config: &mut owner.spec.config,
            prepared: self,
        })
    }
}
pub struct BoundLinkSysConfig<'a> {
    config: &'a SystemConfig,
    prepared: &'a PreparedLinkSysReferences,
}
pub struct BoundLinkSysConfigMut<'a> {
    config: &'a mut SystemConfig,
    prepared: &'a PreparedLinkSysReferences,
}
macro_rules! access {
    () => {
        /// Actual source scope, validated even when no reference or TLS is active.
        pub fn source(&self) -> Result<super::ReferenceSource<'_>, ReferenceError> {
            self.prepared.snapshot.owner.source()
        }
        pub fn config(&self) -> &SystemConfig {
            self.config
        }
        pub fn references(&self) -> Result<LinkSysReferences<'_>, ReferenceError> {
            if !self.prepared.snapshot.config.matches(self.config) {
                return Err(ReferenceError::StaleBinding);
            }
            Ok(LinkSysReferences {
                prepared: &self.prepared.snapshot.config,
                topology: &self.prepared.snapshot,
            })
        }
        pub fn outbound_tls(&self) -> Result<Option<super::outbound_tls::BoundOutboundTls<'_>>, ReferenceError> {
            let references = self.references()?;
            if references.tls().is_none() {
                return Ok(None);
            }
            link_sys_tls_field(references, self.config).map(Some)
        }
        pub fn outbound_tls_selection(
            &self,
        ) -> Result<super::outbound_tls::BoundLocalTlsSelection<'_>, ReferenceError> {
            let references = self.references()?;
            if references.tls().is_some() {
                Ok(super::outbound_tls::BoundLocalTlsSelection::from_active(
                    link_sys_tls_field(references, self.config)?,
                ))
            } else {
                Ok(super::outbound_tls::BoundLocalTlsSelection::from_link_sys(
                    references,
                ))
            }
        }
    };
}
impl BoundLinkSysConfig<'_> {
    access!();
}
impl BoundLinkSysConfigMut<'_> {
    access!();
    pub fn config_mut(&mut self) -> &mut SystemConfig {
        self.config
    }
}
pub struct LinkSysReferences<'a> {
    prepared: &'a PreparedConfig,
    topology: &'a Arc<LinkSysSnapshot>,
}
pub struct LinkSysTlsReferences<'a> {
    references: &'a LinkSysReferences<'a>,
}

pub(super) struct LinkSysTlsField<'a> {
    references: LinkSysReferences<'a>,
    tls: &'a TlsConfig,
}
impl LinkSysTlsField<'_> {
    pub(super) fn validate_source(&self) -> Result<(), ReferenceError> {
        self.references.validate_source()
    }
    pub(super) fn policy(&self) -> &crate::resources::common::OutboundTlsLocal {
        &self.tls.outbound
    }
    pub(super) fn ca_material(&self) -> Option<&[k8s_openapi::api::core::v1::Secret]> {
        self.tls.resolved_ca_certificates.as_deref()
    }
    pub(super) fn client_material(&self) -> Option<&k8s_openapi::api::core::v1::Secret> {
        self.tls.resolved_client_certificate.as_ref()
    }
    pub(super) fn visit_ca<'loan>(&'loan self, mut visit: impl FnMut(ReferenceToken<'loan>)) {
        // Preserve every declared slot. Controller UnsupportedSkipped behavior
        // does not turn an unsupported target into a proved Secret reference.
        for atom in &self.references.prepared.atoms {
            if let PreparedAtom::Reference {
                role: Role::Ca, slot, ..
            } = atom
            {
                visit(slot.token(TopologyLoan::LinkSys(self.references.topology)));
            }
        }
    }
    pub(super) fn client_reference(&self) -> Option<ReferenceToken<'_>> {
        self.references.single(Role::Client)
    }
}
fn link_sys_tls_field<'a>(
    references: LinkSysReferences<'a>,
    config: &'a SystemConfig,
) -> Result<super::outbound_tls::BoundOutboundTls<'a>, ReferenceError> {
    let tls = current_tls(config)
        .filter(|tls| tls.enabled)
        .ok_or(ReferenceError::StaleBinding)?;
    Ok(super::outbound_tls::BoundOutboundTls::from_link_sys(LinkSysTlsField {
        references,
        tls,
    }))
}
fn current_tls(config: &SystemConfig) -> Option<&TlsConfig> {
    match config {
        SystemConfig::Redis(config) => config.tls.as_ref(),
        SystemConfig::Etcd(config) => config.tls.as_ref(),
        SystemConfig::Elasticsearch(config) => config.tls.as_ref(),
        SystemConfig::Kafka(config) => config.tls.as_ref(),
        SystemConfig::Otlp(config) => config.tls.as_ref(),
        SystemConfig::Webhook(config) => config.conn.tls.as_ref(),
        SystemConfig::HttpDns(config) => config
            .connection
            .as_ref()
            .and_then(|connection| connection.tls.as_ref()),
        SystemConfig::CredentialSource(config) => match &config.provider {
            CredentialSourceProvider::OAuth2ClientCredentials(provider) => provider.tls.as_ref(),
        },
        SystemConfig::Unsupported => None,
    }
}
/// Existing providers skip well-formed unsupported CA target kinds; OTLP
/// reports them as invalid. Neither case queries the Secret store for that kind.
pub enum LinkSysCaSlot<'a> {
    Secret(ReferenceToken<'a>),
    UnsupportedSkipped,
}
impl LinkSysReferences<'_> {
    pub(super) fn validate_source(&self) -> Result<(), ReferenceError> {
        self.topology.owner.source().map(|_| ())
    }
    pub fn request_secrets(&self) -> impl Iterator<Item = super::TemplateSecretReference<'_>> {
        self.prepared.atoms.iter().filter_map(move |atom| match atom {
            PreparedAtom::Template(atom) => atom.reference(TopologyLoan::LinkSys(self.topology)),
            _ => None,
        })
    }
    fn single(&self, role: Role) -> Option<ReferenceToken<'_>> {
        self.prepared.atoms.iter().find_map(|atom| match atom {
            PreparedAtom::Reference {
                role: current, slot, ..
            } if *current == role => Some(slot.token(TopologyLoan::LinkSys(self.topology))),
            _ => None,
        })
    }
    pub fn auth_secret(&self) -> Option<ReferenceToken<'_>> {
        self.single(Role::Auth)
    }
    pub fn active_bootstrap_secret(&self) -> Option<ReferenceToken<'_>> {
        self.single(Role::Active)
    }
    pub fn previous_bootstrap_secret(&self) -> Option<ReferenceToken<'_>> {
        self.single(Role::Previous)
    }
    pub fn tls(&self) -> Option<LinkSysTlsReferences<'_>> {
        self.prepared
            .atoms
            .iter()
            .any(|atom| matches!(atom, PreparedAtom::Presence(Role::Enabled, true)))
            .then_some(LinkSysTlsReferences { references: self })
    }
}
impl LinkSysTlsReferences<'_> {
    pub fn ca_certificates(&self) -> impl Iterator<Item = LinkSysCaSlot<'_>> {
        self.references
            .prepared
            .atoms
            .iter()
            .filter_map(move |atom| match atom {
                PreparedAtom::Reference {
                    role: Role::Ca, slot, ..
                } => Some(
                    if slot.unsupported_target()
                        && self.references.prepared.provider != Provider::Otlp
                        && matches!(&slot.wire, FieldWire::Ca(wire) if wire.kind != "Secret")
                    {
                        LinkSysCaSlot::UnsupportedSkipped
                    } else {
                        LinkSysCaSlot::Secret(slot.token(TopologyLoan::LinkSys(self.references.topology)))
                    },
                ),
                _ => None,
            })
    }
    pub fn client_certificate(&self) -> Option<ReferenceToken<'_>> {
        self.references.single(Role::Client)
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/link_sys_fields_test.rs"]
mod tests;
