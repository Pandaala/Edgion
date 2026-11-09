//! Actual EdgionPlugins credential fields, prepared once and lent under owner borrows.
//!
//! Credential accessors take no raw reference, field index, profile or source identity.
//! Mirror accessors index the actual owning resource without extending credential topology.
use super::owned::{topology_revision, TopologyLoan};
use super::owned::{FieldContract, FieldSlot, FieldWire, OwnerSnapshot, PreparedSlot};
use super::template_fields::{PreparedTemplateAtom, TemplateAtom};
use super::{ReferenceError, ReferenceToken};
use crate::resources::common::{CustomCACertificateRef, OutboundTlsLocal, SecretObjectReference, TlsConfig};
use crate::resources::edgion_plugins::{applicable_stages_by_name, EdgionPlugin, EdgionPlugins};
use serde::Serialize;
use std::mem::{discriminant, Discriminant};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(super) enum PluginStage {
    Request,
    ResponseFilter,
    ResponseBodyFilter,
    Response,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(super) enum Role {
    Groups,
    HeaderCa,
    Csrf,
    Jwe,
    OidcClient,
    OidcSession,
    LdapBind,
    WasmAuth,
    WasmPull,
    Credential,
    Tls,
    TlsEnabled,
    Validation,
    Ca,
    Client,
    Evaluator,
    Descriptor,
}

impl Role {
    fn contract(self) -> FieldContract {
        match self {
            Self::Ca => FieldContract::CustomCa,
            Self::Credential => FieldContract::CustomCredential,
            Self::Descriptor => FieldContract::FixedConfigMap,
            _ => FieldContract::CustomSecret,
        }
    }
}

enum Atom<'a> {
    ManagedTarget(&'a crate::resources::common::external_http::WebhookTarget),
    LinkSys {
        namespace: Option<&'a str>,
        name: &'a str,
    },
    Descriptor {
        name: &'a str,
        key: &'a str,
    },
    Template(TemplateAtom),
    Presence(Role, bool),
    Group {
        ordinal: usize,
        name: &'a str,
    },
    Secret {
        role: Role,
        group: Option<usize>,
        ordinal: usize,
        wire: &'a SecretObjectReference,
    },
    Ca {
        ordinal: usize,
        wire: &'a CustomCACertificateRef,
    },
}

#[derive(PartialEq, Eq, Serialize)]
enum PreparedAtom {
    ManagedTarget {
        url: bool,
        group: Option<String>,
        kind: Option<String>,
        namespace: Option<String>,
        name: Option<String>,
        port: Option<i32>,
    },
    LinkSys {
        namespace: Option<String>,
        name: String,
    },
    Template(PreparedTemplateAtom),
    Presence(Role, bool),
    Group {
        ordinal: usize,
        name: String,
    },
    Reference {
        role: Role,
        group: Option<usize>,
        ordinal: usize,
        slot: Box<PreparedSlot>,
    },
}

impl PreparedAtom {
    fn matches(&self, stage: PluginStage, index: usize, atom: &Atom<'_>) -> bool {
        match (self, atom) {
            (
                Self::ManagedTarget {
                    url,
                    group,
                    kind,
                    namespace,
                    name,
                    port,
                },
                Atom::ManagedTarget(current),
            ) => {
                *url == current.url.is_some()
                    && *group == current.service.group
                    && *kind == current.service.kind
                    && *namespace == current.service.namespace
                    && *name == current.service.name
                    && *port == current.service.port
            }
            (
                Self::LinkSys { namespace, name },
                Atom::LinkSys {
                    namespace: current_namespace,
                    name: current_name,
                },
            ) => namespace.as_deref() == *current_namespace && name == current_name,
            (
                Self::Reference {
                    role: Role::Descriptor,
                    group: None,
                    ordinal: 0,
                    slot,
                },
                Atom::Descriptor { name, key },
            ) => {
                slot.matches_plugin(stage, index, Role::Descriptor, None, 0)
                    && matches!(&slot.wire, FieldWire::Descriptor { name: prepared_name, key: prepared_key }
                        if prepared_name.as_str() == *name && prepared_key.as_str() == *key)
            }
            (Self::Template(prepared), Atom::Template(current)) => prepared.matches(current),
            (Self::Presence(role, value), Atom::Presence(current_role, current_value)) => {
                role == current_role && value == current_value
            }
            (
                Self::Group { ordinal, name },
                Atom::Group {
                    ordinal: current_ordinal,
                    name: current_name,
                },
            ) => ordinal == current_ordinal && name == current_name,
            (
                Self::Reference {
                    role,
                    group,
                    ordinal,
                    slot,
                },
                Atom::Secret {
                    role: current_role,
                    group: current_group,
                    ordinal: current_ordinal,
                    wire,
                },
            ) => {
                role == current_role
                    && group == current_group
                    && ordinal == current_ordinal
                    && slot.matches_plugin(stage, index, *role, *group, *ordinal)
                    && matches!(&slot.wire, FieldWire::Secret(prepared) if prepared == *wire)
            }
            (
                Self::Reference {
                    role: Role::Ca,
                    ordinal,
                    slot,
                    ..
                },
                Atom::Ca {
                    ordinal: current_ordinal,
                    wire,
                },
            ) => {
                ordinal == current_ordinal
                    && slot.matches_plugin(stage, index, Role::Ca, None, *ordinal)
                    && matches!(&slot.wire, FieldWire::Ca(prepared) if prepared == *wire)
            }
            _ => false,
        }
    }
}

#[derive(PartialEq, Eq, Serialize)]
struct PreparedEntry {
    stage: PluginStage,
    index: usize,
    #[serde(skip)]
    variant: Discriminant<EdgionPlugin>,
    variant_name: &'static str,
    enabled: bool,
    supported: bool,
    atoms: Vec<PreparedAtom>,
}

impl PreparedEntry {
    fn new(owner: &OwnerSnapshot, stage: PluginStage, index: usize, enabled: bool, plugin: &EdgionPlugin) -> Self {
        let supported = supports_stage(stage, plugin);
        let mut atoms = Vec::new();
        visit_fields(plugin, &mut |atom| {
            atoms.push(match atom {
                Atom::ManagedTarget(target) => PreparedAtom::ManagedTarget {
                    url: target.url.is_some(),
                    group: target.service.group.clone(),
                    kind: target.service.kind.clone(),
                    namespace: target.service.namespace.clone(),
                    name: target.service.name.clone(),
                    port: target.service.port,
                },
                Atom::LinkSys { namespace, name } => PreparedAtom::LinkSys {
                    namespace: namespace.map(str::to_owned),
                    name: name.to_owned(),
                },
                Atom::Descriptor { name, key } => PreparedAtom::Reference {
                    role: Role::Descriptor,
                    group: None,
                    ordinal: 0,
                    slot: Box::new(PreparedSlot::new(
                        owner,
                        FieldWire::Descriptor {
                            name: name.into(),
                            key: key.into(),
                        },
                        FieldContract::FixedConfigMap,
                        FieldSlot::Plugin {
                            stage,
                            entry: index,
                            role: Role::Descriptor,
                            group: None,
                            reference: 0,
                        },
                    )),
                },
                Atom::Template(atom) => {
                    PreparedAtom::Template(PreparedTemplateAtom::new(owner, atom, |position, segment| {
                        FieldSlot::PluginTemplate {
                            stage,
                            entry: index,
                            position,
                            segment,
                        }
                    }))
                }
                Atom::Presence(role, value) => PreparedAtom::Presence(role, value),
                Atom::Group { ordinal, name } => PreparedAtom::Group {
                    ordinal,
                    name: name.to_string(),
                },
                Atom::Secret {
                    role,
                    group,
                    ordinal,
                    wire,
                } => PreparedAtom::Reference {
                    role,
                    group,
                    ordinal,
                    slot: Box::new(PreparedSlot::new(
                        owner,
                        FieldWire::Secret(wire.clone()),
                        role.contract(),
                        FieldSlot::Plugin {
                            stage,
                            entry: index,
                            role,
                            group,
                            reference: ordinal,
                        },
                    )),
                },
                Atom::Ca { ordinal, wire } => PreparedAtom::Reference {
                    role: Role::Ca,
                    group: None,
                    ordinal,
                    slot: Box::new(PreparedSlot::new(
                        owner,
                        FieldWire::Ca(wire.clone()),
                        FieldContract::CustomCa,
                        FieldSlot::Plugin {
                            stage,
                            entry: index,
                            role: Role::Ca,
                            group: None,
                            reference: ordinal,
                        },
                    )),
                },
            });
        });
        Self {
            stage,
            index,
            variant: discriminant(plugin),
            variant_name: plugin.type_name(),
            enabled,
            supported,
            atoms,
        }
    }
    fn matches(&self, enabled: bool, plugin: &EdgionPlugin) -> bool {
        if self.variant != discriminant(plugin) || self.enabled != enabled {
            return false;
        }
        let mut expected = self.atoms.iter();
        let mut matches = true;
        visit_fields(plugin, &mut |atom| {
            matches &= expected
                .next()
                .is_some_and(|prepared| prepared.matches(self.stage, self.index, &atom));
        });
        matches && expected.next().is_none()
    }
}

pub(super) fn supports_stage(stage: PluginStage, plugin: &EdgionPlugin) -> bool {
    let stages = applicable_stages_by_name(plugin.type_name());
    match stage {
        PluginStage::Request => stages.request,
        PluginStage::ResponseFilter => stages.upstream_response_filter,
        PluginStage::ResponseBodyFilter => stages.upstream_response_body_filter,
        PluginStage::Response => stages.upstream_response,
    }
}

fn single<'a>(role: Role, wire: Option<&'a SecretObjectReference>, visitor: &mut impl FnMut(Atom<'a>)) {
    visitor(Atom::Presence(role, wire.is_some()));
    if let Some(wire) = wire {
        visitor(Atom::Secret {
            role,
            group: None,
            ordinal: 0,
            wire,
        });
    }
}

fn tls<'a>(wire: Option<&'a OutboundTlsLocal>, enabled: Option<bool>, visitor: &mut impl FnMut(Atom<'a>)) {
    visitor(Atom::Presence(Role::Tls, wire.is_some()));
    visitor(Atom::Presence(Role::TlsEnabled, enabled.unwrap_or(true)));
    if let Some(wire) = wire {
        visitor(Atom::Presence(Role::Validation, wire.validation.is_some()));
        let cas = wire
            .validation
            .as_ref()
            .and_then(|validation| validation.ca_certificate_refs.as_ref());
        visitor(Atom::Presence(Role::Ca, cas.is_some()));
        for (ordinal, wire) in cas.into_iter().flatten().enumerate() {
            visitor(Atom::Ca { ordinal, wire });
        }
        single(Role::Client, wire.client_certificate_ref.as_ref(), visitor);
    }
}

fn switched_tls<'a>(wire: Option<&'a TlsConfig>, visitor: &mut impl FnMut(Atom<'a>)) {
    tls(wire.map(|wire| &wire.outbound), wire.map(|wire| wire.enabled), visitor);
}

// This one borrowed walker builds snapshots and compares current fields. It
// never traverses or clones resolved credentials, keys or certificate material.
fn visit_fields<'a>(plugin: &'a EdgionPlugin, visitor: &mut impl FnMut(Atom<'a>)) {
    macro_rules! groups {
        ($groups:expr) => {{
            let groups = $groups;
            visitor(Atom::Presence(Role::Groups, groups.is_some()));
            for (group, value) in groups.into_iter().flatten().enumerate() {
                visitor(Atom::Group {
                    ordinal: group,
                    name: &value.name,
                });
                for (ordinal, wire) in value.secret_refs.iter().enumerate() {
                    visitor(Atom::Secret {
                        role: Role::Groups,
                        group: Some(group),
                        ordinal,
                        wire,
                    });
                }
            }
        }};
    }
    match plugin {
        EdgionPlugin::Dsl(config) => {
            for reference in &config.linksys_refs {
                visitor(Atom::LinkSys {
                    namespace: Some(&reference.namespace),
                    name: &reference.name,
                });
            }
        }
        EdgionPlugin::RateLimitRedis(config) => visitor(Atom::LinkSys {
            namespace: None,
            name: &config.redis_ref,
        }),
        EdgionPlugin::GraphqlRateLimit(config) => visitor(Atom::LinkSys {
            namespace: None,
            name: &config.redis_ref,
        }),
        EdgionPlugin::WebhookKeyGet(config) => {
            for var in &config.vars {
                visitor(Atom::LinkSys {
                    namespace: None,
                    name: &var.webhook_ref,
                });
            }
        }
        EdgionPlugin::AiProxy(config) => {
            if let Some(quota) = &config.token_quota {
                visitor(Atom::LinkSys {
                    namespace: None,
                    name: &quota.redis_ref,
                });
            }
        }
        EdgionPlugin::GrpcJsonTranscoder(config) => {
            if let crate::grpc_transcoding::GrpcDescriptorSource::ConfigMapRef { name, key } = &config.descriptor {
                visitor(Atom::Descriptor { name, key });
            }
        }
        EdgionPlugin::BasicAuth(config) => groups!(config.secret_groups.as_ref()),
        EdgionPlugin::KeyAuth(config) => groups!(config.secret_groups.as_ref()),
        EdgionPlugin::JwtAuth(config) => groups!(config.issuer_groups.as_ref()),
        EdgionPlugin::HmacAuth(config) => groups!(config.match_config.secret_groups.as_ref()),
        EdgionPlugin::HeaderCertAuth(config) => {
            visitor(Atom::Presence(
                Role::HeaderCa,
                config.mode == crate::resources::edgion_plugins::CertSourceMode::Header,
            ));
            for (ordinal, wire) in config.ca_secret_refs.iter().enumerate() {
                visitor(Atom::Secret {
                    role: Role::HeaderCa,
                    group: None,
                    ordinal,
                    wire,
                });
            }
        }
        EdgionPlugin::Csrf(config) => single(Role::Csrf, config.secret_ref.as_ref(), visitor),
        EdgionPlugin::JweDecrypt(config) => single(Role::Jwe, config.secret_ref.as_ref(), visitor),
        EdgionPlugin::OpenidConnect(config) => {
            single(Role::OidcClient, config.client_secret_ref.as_ref(), visitor);
            single(Role::OidcSession, config.session_secret_ref.as_ref(), visitor);
            tls(Some(&config.tls), None, visitor);
        }
        EdgionPlugin::LdapAuth(config) => {
            single(Role::LdapBind, config.bind_password_ref.as_ref(), visitor);
            tls(Some(&config.tls), None, visitor);
        }
        EdgionPlugin::ExtProc(config) => switched_tls(Some(&config.grpc_service.tls), visitor),
        EdgionPlugin::Wasm(config) => {
            visitor(Atom::Presence(Role::WasmAuth, config.source.fetch.is_some()));
            if let Some(fetch) = &config.source.fetch {
                single(
                    Role::WasmAuth,
                    fetch.auth_header_secret_ref.as_ref().map(|reference| &reference.secret),
                    visitor,
                );
                switched_tls(fetch.tls.as_ref(), visitor);
            }
            visitor(Atom::Presence(Role::WasmPull, config.source.oci.is_some()));
            if let Some(oci) = &config.source.oci {
                single(Role::WasmPull, oci.pull_secret_ref.as_ref(), visitor);
            }
        }
        EdgionPlugin::CredentialInjector(config) => single(Role::Credential, Some(&config.credential_ref), visitor),
        EdgionPlugin::ForwardAuth(config) => {
            visitor(Atom::ManagedTarget(&config.conn.target));
            switched_tls(config.conn.tls.as_ref(), visitor);
            super::template_fields::visit(&config.request, |atom| visitor(Atom::Template(atom)));
        }
        EdgionPlugin::Guardrail(config) => {
            visitor(Atom::ManagedTarget(&config.conn.target));
            switched_tls(config.conn.tls.as_ref(), visitor);
            super::template_fields::visit(&config.request, |atom| visitor(Atom::Template(atom)));
        }
        EdgionPlugin::AiGuard(config) => {
            visitor(Atom::Presence(Role::Evaluator, config.evaluator.is_some()));
            if let Some(evaluator) = &config.evaluator {
                visitor(Atom::ManagedTarget(&evaluator.conn.target));
                switched_tls(evaluator.conn.tls.as_ref(), visitor);
                super::template_fields::visit(&evaluator.request, |atom| visitor(Atom::Template(atom)));
            }
        }
        _ => {}
    }
}

/// A cold snapshot of actual credential fields in all four existing stages.
/// No material is retained and no lookup, authorization or dependency write runs.
pub struct PreparedPluginReferences {
    snapshot: Arc<PluginSnapshot>,
}

#[derive(PartialEq, Eq, Serialize)]
pub(super) struct PluginSnapshot {
    pub(super) owner: OwnerSnapshot,
    #[serde(skip)]
    pub(super) revision: String,
    present: [bool; 4],
    stages: [Vec<PreparedEntry>; 4],
}

impl PreparedPluginReferences {
    pub fn from_plugins(owner: &EdgionPlugins) -> Self {
        let source = OwnerSnapshot::namespaced("edgion.io", "EdgionPlugins", &owner.metadata);
        macro_rules! stage {
            ($entries:expr, $stage:expr) => {
                $entries
                    .as_ref()
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .map(|(index, entry)| PreparedEntry::new(&source, $stage, index, entry.enable, &entry.plugin))
                    .collect()
            };
        }
        let mut snapshot = PluginSnapshot {
            stages: [
                stage!(owner.spec.request_plugins, PluginStage::Request),
                stage!(owner.spec.upstream_response_filter_plugins, PluginStage::ResponseFilter),
                stage!(
                    owner.spec.upstream_response_body_filter_plugins,
                    PluginStage::ResponseBodyFilter
                ),
                stage!(owner.spec.upstream_response_plugins, PluginStage::Response),
            ],
            present: [
                owner.spec.request_plugins.is_some(),
                owner.spec.upstream_response_filter_plugins.is_some(),
                owner.spec.upstream_response_body_filter_plugins.is_some(),
                owner.spec.upstream_response_plugins.is_some(),
            ],
            owner: source,
            revision: String::new(),
        };
        snapshot.revision = topology_revision(&snapshot);
        Self {
            snapshot: Arc::new(snapshot),
        }
    }
    pub fn bind<'a>(&'a self, owner: &'a EdgionPlugins) -> Result<PluginBindings<'a>, ReferenceError> {
        if !self.snapshot.matches(owner) {
            return Err(ReferenceError::StaleBinding);
        }
        Ok(PluginBindings { owner, prepared: self })
    }
    pub fn bind_mut<'a>(&'a self, owner: &'a mut EdgionPlugins) -> Result<PluginBindingsMut<'a>, ReferenceError> {
        if !self.snapshot.matches(owner) {
            return Err(ReferenceError::StaleBinding);
        }
        Ok(PluginBindingsMut { owner, prepared: self })
    }
}

impl PluginSnapshot {
    pub(super) fn matches(&self, owner: &EdgionPlugins) -> bool {
        if !self.owner.matches(&owner.metadata) {
            return false;
        }
        macro_rules! stage {
            ($entries:expr, $index:expr) => {{
                let entries = $entries.as_ref();
                self.present[$index] == entries.is_some()
                    && entries.map_or(0, Vec::len) == self.stages[$index].len()
                    && entries
                        .into_iter()
                        .flatten()
                        .zip(&self.stages[$index])
                        .all(|(entry, prepared)| prepared.matches(entry.enable, &entry.plugin))
            }};
        }
        stage!(owner.spec.request_plugins, 0)
            && stage!(owner.spec.upstream_response_filter_plugins, 1)
            && stage!(owner.spec.upstream_response_body_filter_plugins, 2)
            && stage!(owner.spec.upstream_response_plugins, 3)
    }
    pub(super) fn slot(&self, position: &FieldSlot) -> Option<&PreparedSlot> {
        let (FieldSlot::Plugin { stage, entry, .. } | FieldSlot::PluginTemplate { stage, entry, .. }) = position else {
            return None;
        };
        let stage = match stage {
            PluginStage::Request => 0,
            PluginStage::ResponseFilter => 1,
            PluginStage::ResponseBodyFilter => 2,
            PluginStage::Response => 3,
        };
        self.stages
            .get(stage)?
            .get(*entry)?
            .atoms
            .iter()
            .find_map(|atom| match atom {
                PreparedAtom::Reference { slot, .. } if slot.position() == position => Some(slot.as_ref()),
                PreparedAtom::Template(atom) => atom.slot().filter(|slot| slot.position() == position),
                _ => None,
            })
    }
}

pub struct PluginBindings<'a> {
    owner: &'a EdgionPlugins,
    prepared: &'a PreparedPluginReferences,
}
pub struct PluginBindingsMut<'a> {
    owner: &'a mut EdgionPlugins,
    prepared: &'a PreparedPluginReferences,
}

macro_rules! readonly_stage {
    ($method:ident, $field:ident, $index:expr) => {
        pub fn $method(&self) -> impl Iterator<Item = BoundPluginEntry<'_>> {
            self.owner
                .spec
                .$field
                .as_ref()
                .into_iter()
                .flatten()
                .zip(&self.prepared.snapshot.stages[$index])
                .map(|(entry, prepared)| BoundPluginEntry {
                    plugin: &entry.plugin,
                    enabled: entry.enable,
                    prepared,
                    topology: &self.prepared.snapshot,
                })
        }
    };
}
macro_rules! mutable_stage {
    ($method:ident, $field:ident, $index:expr) => {
        pub fn $method(&mut self) -> impl Iterator<Item = BoundPluginEntryMut<'_>> {
            self.owner
                .spec
                .$field
                .as_mut()
                .into_iter()
                .flatten()
                .zip(&self.prepared.snapshot.stages[$index])
                .map(|(entry, prepared)| BoundPluginEntryMut {
                    plugin: &mut entry.plugin,
                    enabled: entry.enable,
                    prepared,
                    topology: &self.prepared.snapshot,
                })
        }
    };
}
impl PluginBindings<'_> {
    readonly_stage!(request_entries, request_plugins, 0);
    readonly_stage!(response_filter_entries, upstream_response_filter_plugins, 1);
    readonly_stage!(response_body_filter_entries, upstream_response_body_filter_plugins, 2);
    readonly_stage!(response_entries, upstream_response_plugins, 3);
}
impl PluginBindingsMut<'_> {
    mutable_stage!(request_entries, request_plugins, 0);
    mutable_stage!(response_filter_entries, upstream_response_filter_plugins, 1);
    mutable_stage!(response_body_filter_entries, upstream_response_body_filter_plugins, 2);
    mutable_stage!(response_entries, upstream_response_plugins, 3);
}

/// A borrowed configuration variant view, not an independently unforgeable proof.
/// Source provenance comes from `BoundPluginEntry::link_sys_config` and its caller;
/// target grammar and other configuration validation remain owned by each schema.
pub enum HttpLinkSysConfig<'a> {
    Dsl(&'a crate::resources::edgion_plugins::DslConfig),
    RateLimitRedis(&'a crate::resources::edgion_plugins::RateLimitRedisConfig),
    GraphqlRateLimit(&'a crate::resources::edgion_plugins::GraphqlRateLimitConfig),
    WebhookKeyGet(&'a crate::resources::edgion_plugins::WebhookKeyGetConfig),
    AiProxy(&'a crate::resources::edgion_plugins::AiProxyConfig),
}

impl<'a> HttpLinkSysConfig<'a> {
    fn from_plugin(plugin: &'a EdgionPlugin) -> Option<Self> {
        match plugin {
            EdgionPlugin::Dsl(config) if !config.linksys_refs.is_empty() => Some(Self::Dsl(config)),
            EdgionPlugin::RateLimitRedis(config) => Some(Self::RateLimitRedis(config)),
            EdgionPlugin::GraphqlRateLimit(config) => Some(Self::GraphqlRateLimit(config)),
            EdgionPlugin::WebhookKeyGet(config) if !config.vars.is_empty() => Some(Self::WebhookKeyGet(config)),
            EdgionPlugin::AiProxy(config) if config.token_quota.is_some() => Some(Self::AiProxy(config)),
            _ => None,
        }
    }

    /// Classifies presence only; this does not establish an owning loan.
    pub fn is_reference_bearing(plugin: &'a EdgionPlugin) -> bool {
        Self::from_plugin(plugin).is_some()
    }
}

/// An actual managed HTTP plugin configuration. Private fields bind the config,
/// Service target and TLS selection to one owner and entry borrow.
///
/// ```compile_fail
/// use edgion_resources::{EdgionPlugins, resources::refs::plugin_fields::PreparedPluginReferences};
/// fn owner_cannot_move(owner: EdgionPlugins) {
///     let prepared = PreparedPluginReferences::from_plugins(&owner);
///     let binding = prepared.bind(&owner).unwrap();
///     let entry = binding.request_entries().next().unwrap();
///     let field = entry.managed_http_config().unwrap().unwrap();
///     drop(owner);
///     field.connection();
/// }
/// ```
/// ```compile_fail
/// use edgion_resources::{EdgionPlugins, resources::refs::plugin_fields::PreparedPluginReferences};
/// fn owner_cannot_change(mut owner: EdgionPlugins) {
///     let prepared = PreparedPluginReferences::from_plugins(&owner);
///     let binding = prepared.bind(&owner).unwrap();
///     let entry = binding.request_entries().next().unwrap();
///     let field = entry.managed_http_config().unwrap().unwrap();
///     owner.metadata.namespace = None;
///     field.connection();
/// }
/// ```
pub struct BoundManagedHttpConfig<'a> {
    plugin: &'a EdgionPlugin,
    references: EntryReferences<'a>,
    connection: &'a crate::resources::common::ExternalHttpConnection,
    request: &'a crate::resources::common::OutboundRequestSpec,
    secrets: Option<&'a crate::resources::common::ResolvedSecrets>,
    namespace: &'a str,
}

impl BoundManagedHttpConfig<'_> {
    /// Presence only, never an authority proof.
    pub fn is_reference_bearing(plugin: &EdgionPlugin) -> bool {
        matches!(plugin, EdgionPlugin::ForwardAuth(_) | EdgionPlugin::Guardrail(_))
            || matches!(plugin, EdgionPlugin::AiGuard(config) if config.evaluator.is_some())
    }
    pub fn plugin(&self) -> &EdgionPlugin {
        self.plugin
    }
    pub fn connection(&self) -> &crate::resources::common::ExternalHttpConnection {
        self.connection
    }
    pub fn request(&self) -> &crate::resources::common::OutboundRequestSpec {
        self.request
    }
    pub fn resolved_secrets(&self) -> Option<&crate::resources::common::ResolvedSecrets> {
        self.secrets
    }
    pub fn owner_namespace(&self) -> &str {
        self.namespace
    }
    pub fn outbound_tls_selection(&self) -> Result<super::outbound_tls::BoundLocalTlsSelection<'_>, ReferenceError> {
        let references = EntryReferences {
            prepared: self.references.prepared,
            topology: self.references.topology,
        };
        if references.tls().is_some() {
            Ok(super::outbound_tls::BoundLocalTlsSelection::from_active(
                plugin_tls_field(references, self.plugin)?,
            ))
        } else {
            Ok(super::outbound_tls::BoundLocalTlsSelection::from_plugin(references))
        }
    }
}

pub struct BoundPluginEntry<'a> {
    plugin: &'a EdgionPlugin,
    enabled: bool,
    prepared: &'a PreparedEntry,
    topology: &'a Arc<PluginSnapshot>,
}
/// A mutable entry whose capabilities cannot outlive its current immutable loan.
///
/// ```compile_fail
/// use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;
/// use edgion_resources::EdgionPlugins;
/// let mut owner: EdgionPlugins = serde_json::from_value(serde_json::json!({
///     "apiVersion":"edgion.io/v1", "kind":"EdgionPlugins",
///     "metadata":{"name":"p"}, "spec":{"requestPlugins":[{
///         "type":"Csrf", "config":{"secretRef":{"name":"key"}}
///     }]}
/// })).unwrap();
/// let prepared = PreparedPluginReferences::from_plugins(&owner);
/// let mut bound = prepared.bind_mut(&mut owner).unwrap();
/// let mut entry = bound.request_entries().next().unwrap();
/// let references = entry.references().unwrap();
/// let token = references.csrf_secret().unwrap();
/// entry.config_mut();
/// token.validated().unwrap();
/// ```
/// Derived grant queries also retain the current config loan.
///
/// ```compile_fail
/// use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;
/// use edgion_resources::EdgionPlugins;
/// let mut owner: EdgionPlugins = serde_json::from_value(serde_json::json!({
///     "apiVersion":"edgion.io/v1", "kind":"EdgionPlugins",
///     "metadata":{"name":"p"}, "spec":{"requestPlugins":[{
///         "type":"Csrf", "config":{"secretRef":{"name":"key"}}
///     }]}
/// })).unwrap();
/// let prepared = PreparedPluginReferences::from_plugins(&owner);
/// let mut bound = prepared.bind_mut(&mut owner).unwrap();
/// let mut entry = bound.request_entries().next().unwrap();
/// let references = entry.references().unwrap();
/// let token = references.csrf_secret().unwrap();
/// let reference = token.validated().unwrap();
/// let query = reference.grant_query();
/// entry.config_mut();
/// query.target();
/// ```
/// The actual owner cannot be dropped while a field capability remains live.
///
/// ```compile_fail
/// use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;
/// use edgion_resources::EdgionPlugins;
/// let mut owner: EdgionPlugins = serde_json::from_value(serde_json::json!({
///     "apiVersion":"edgion.io/v1", "kind":"EdgionPlugins",
///     "metadata":{"name":"p"}, "spec":{"requestPlugins":[{
///         "type":"Csrf", "config":{"secretRef":{"name":"key"}}
///     }]}
/// })).unwrap();
/// let prepared = PreparedPluginReferences::from_plugins(&owner);
/// let mut bound = prepared.bind_mut(&mut owner).unwrap();
/// let mut entry = bound.request_entries().next().unwrap();
/// let references = entry.references().unwrap();
/// let token = references.csrf_secret().unwrap();
/// drop(owner);
/// token.validated().unwrap();
/// ```
/// A second mutable owner binding cannot overlap a live capability.
///
/// ```compile_fail
/// use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;
/// use edgion_resources::EdgionPlugins;
/// let mut owner: EdgionPlugins = serde_json::from_value(serde_json::json!({
///     "apiVersion":"edgion.io/v1", "kind":"EdgionPlugins",
///     "metadata":{"name":"p"}, "spec":{"requestPlugins":[{
///         "type":"Csrf", "config":{"secretRef":{"name":"key"}}
///     }]}
/// })).unwrap();
/// let prepared = PreparedPluginReferences::from_plugins(&owner);
/// let mut bound = prepared.bind_mut(&mut owner).unwrap();
/// let mut entry = bound.request_entries().next().unwrap();
/// let references = entry.references().unwrap();
/// let token = references.csrf_secret().unwrap();
/// prepared.bind_mut(&mut owner).unwrap();
/// token.validated().unwrap();
/// ```
pub struct BoundPluginEntryMut<'a> {
    plugin: &'a mut EdgionPlugin,
    enabled: bool,
    prepared: &'a PreparedEntry,
    topology: &'a Arc<PluginSnapshot>,
}
/// Actual HeaderCertAuth configuration and its compressed successful CA inputs.
///
/// ```
/// use edgion_resources::{EdgionPlugins, resources::refs::plugin_fields::PreparedPluginReferences};
/// fn inspect(owner: EdgionPlugins) {
///     let prepared = PreparedPluginReferences::from_plugins(&owner);
///     let binding = prepared.bind(&owner).unwrap();
///     let entry = binding.request_entries().next().unwrap();
///     let field = entry.header_cert_auth().unwrap().unwrap();
///     let _ = field.ca_materials().count();
/// }
/// ```
/// ```compile_fail,E0505
/// use edgion_resources::{EdgionPlugins, resources::refs::plugin_fields::PreparedPluginReferences};
/// fn cannot_drop(owner: EdgionPlugins) {
///     let prepared = PreparedPluginReferences::from_plugins(&owner);
///     let binding = prepared.bind(&owner).unwrap();
///     let entry = binding.request_entries().next().unwrap();
///     let field = entry.header_cert_auth().unwrap().unwrap();
///     drop(owner);
///     let _ = field.ca_materials().count();
/// }
/// ```
/// ```compile_fail,E0506
/// use edgion_resources::{EdgionPlugins, resources::refs::plugin_fields::PreparedPluginReferences};
/// fn cannot_mutate(mut owner: EdgionPlugins) {
///     let prepared = PreparedPluginReferences::from_plugins(&owner);
///     let binding = prepared.bind(&owner).unwrap();
///     let entry = binding.request_entries().next().unwrap();
///     let field = entry.header_cert_auth().unwrap().unwrap();
///     owner.metadata.name = None;
///     let _ = field.ca_materials().count();
/// }
/// ```
pub struct BoundHeaderCertAuth<'a> {
    config: &'a crate::resources::edgion_plugins::HeaderCertAuthConfig,
    references: EntryReferences<'a>,
}

impl BoundHeaderCertAuth<'_> {
    pub fn config(&self) -> &crate::resources::edgion_plugins::HeaderCertAuthConfig {
        self.config
    }

    pub fn owner_namespace(&self) -> &str {
        self.references.topology.owner.namespace().unwrap_or("default")
    }

    /// Each carried sibling independently matches an actual declared CA target.
    /// The Controller omits failed references, so material ordinals are not slots.
    pub fn ca_materials(&self) -> impl Iterator<Item = super::outbound_tls::BoundTlsMaterial<'_>> {
        self.config
            .resolved_ca_secrets
            .iter()
            .flatten()
            .filter_map(move |secret| {
                self.references.header_ca_secrets().find_map(|token| {
                    let pair = super::outbound_tls::BoundTlsMaterial::from_header_ca(token, secret);
                    pair.validate_identity().is_ok().then_some(pair)
                })
            })
    }

    /// EntryCache is generation-local. Rejected/absent material must rebuild a
    /// partial or empty pool instead of hiding behind a populated cache slot.
    pub fn cache_reusable(&self) -> bool {
        self.config.mode == crate::resources::edgion_plugins::CertSourceMode::Connection
            || self
                .config
                .resolved_ca_secrets
                .as_ref()
                .is_some_and(|secrets| !secrets.is_empty() && self.ca_materials().count() == secrets.len())
    }
}

macro_rules! entry_access {
    () => {
        pub fn config(&self) -> &EdgionPlugin {
            self.plugin
        }
        /// Borrow the canonical namespace of the actual current entry owner.
        pub fn owner_namespace(&self) -> Result<&str, ReferenceError> {
            self.references()?.validate_source()?;
            self.topology
                .owner
                .namespace()
                .ok_or(ReferenceError::InvalidNamespace)
        }
        /// Lend actual fixed LinkSys configuration after current topology/source checks.
        ///
        /// ```compile_fail
        /// use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;
        /// use edgion_resources::EdgionPlugins;
        /// let owner: EdgionPlugins = serde_json::from_value(serde_json::json!({
        ///     "metadata":{"name":"plugins"}, "spec":{"requestPlugins":[{
        ///         "type":"RateLimitRedis", "config":{"redisRef":"apps/cache", "quotas":[]}
        ///     }]}
        /// })).unwrap();
        /// let prepared = PreparedPluginReferences::from_plugins(&owner);
        /// let bindings = prepared.bind(&owner).unwrap();
        /// let entry = bindings.request_entries().next().unwrap();
        /// let config = entry.link_sys_config().unwrap();
        /// drop(owner);
        /// drop(config);
        /// ```
        pub fn link_sys_config(&self) -> Result<Option<HttpLinkSysConfig<'_>>, ReferenceError> {
            let references = self.references()?;
            if !self.prepared.supported || !self.enabled {
                return Ok(None);
            }
            let config = HttpLinkSysConfig::from_plugin(self.plugin);
            if config.is_some() {
                references.validate_source()?;
            }
            Ok(config)
        }
        pub fn managed_http_config(&self) -> Result<Option<BoundManagedHttpConfig<'_>>, ReferenceError> {
            let references = self.references()?;
            if !self.prepared.supported
                || !self.enabled
                || self.prepared.stage != PluginStage::Request
                || !BoundManagedHttpConfig::is_reference_bearing(self.plugin)
            {
                return Ok(None);
            }
            references.validate_source()?;
            let (connection, request, secrets) = match &*self.plugin {
                EdgionPlugin::ForwardAuth(config) => (&config.conn, &config.request, config.resolved_secrets.as_ref()),
                EdgionPlugin::Guardrail(config) => (&config.conn, &config.request, config.resolved_secrets.as_ref()),
                EdgionPlugin::AiGuard(config) => {
                    let Some(evaluator) = &config.evaluator else {
                        return Ok(None);
                    };
                    (
                        &evaluator.conn,
                        &evaluator.request,
                        evaluator.resolved_secrets.as_ref(),
                    )
                }
                _ => return Ok(None),
            };
            let namespace = references
                .topology
                .owner
                .namespace()
                .ok_or(ReferenceError::InvalidNamespace)?;
            let field = BoundManagedHttpConfig {
                plugin: self.plugin,
                references,
                connection,
                request,
                secrets,
                namespace,
            };
            // URL exclusion and mode validation share the schema's implementation.
            let target = field
                .connection()
                .target
                .kind()
                .map_err(|_| ReferenceError::UnsupportedTarget)?;
            if let crate::resources::common::external_http::TargetKind::Service(wire) = target {
                super::targets::managed_http_service_target(wire, field.owner_namespace())?;
            }
            Ok(Some(field))
        }
        pub fn header_cert_auth(&self) -> Result<Option<BoundHeaderCertAuth<'_>>, ReferenceError> {
            let references = self.references()?;
            if !self.prepared.supported || !self.enabled {
                return Ok(None);
            }
            let EdgionPlugin::HeaderCertAuth(config) = &*self.plugin else {
                return Ok(None);
            };
            if config.mode == crate::resources::edgion_plugins::CertSourceMode::Header {
                references.validate_source()?;
            }
            Ok(Some(BoundHeaderCertAuth { config, references }))
        }
        pub fn is_enabled(&self) -> bool {
            self.enabled
        }
        pub fn references(&self) -> Result<EntryReferences<'_>, ReferenceError> {
            if !self.prepared.matches(self.enabled, self.plugin) {
                return Err(ReferenceError::StaleBinding);
            }
            Ok(EntryReferences {
                prepared: self.prepared,
                topology: self.topology,
            })
        }
        pub fn outbound_tls(&self) -> Result<Option<super::outbound_tls::BoundOutboundTls<'_>>, ReferenceError> {
            let references = self.references()?;
            if references.tls().is_none() {
                return Ok(None);
            }
            plugin_tls_field(references, self.plugin).map(Some)
        }
        pub fn outbound_tls_selection(
            &self,
        ) -> Result<super::outbound_tls::BoundLocalTlsSelection<'_>, ReferenceError> {
            let references = self.references()?;
            if references.tls().is_some() {
                Ok(super::outbound_tls::BoundLocalTlsSelection::from_active(
                    plugin_tls_field(references, self.plugin)?,
                ))
            } else {
                Ok(super::outbound_tls::BoundLocalTlsSelection::from_plugin(
                    references,
                ))
            }
        }
    };
}
impl BoundPluginEntry<'_> {
    entry_access!();
}
impl BoundPluginEntryMut<'_> {
    entry_access!();
    pub fn config_mut(&mut self) -> &mut EdgionPlugin {
        self.plugin
    }
}

pub struct EntryReferences<'a> {
    prepared: &'a PreparedEntry,
    topology: &'a Arc<PluginSnapshot>,
}
pub struct SecretGroupReferences<'a> {
    entry: &'a EntryReferences<'a>,
    ordinal: usize,
    name: &'a str,
}
pub struct TlsReferences<'a> {
    entry: &'a EntryReferences<'a>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DescriptorReferenceError {
    Reference(ReferenceError),
    InvalidSelector,
}

/// Checked exact binaryData selector borrowed from the actual plugin field.
/// ```compile_fail
/// use edgion_resources::resources::refs::plugin_fields::GrpcDescriptorReference;
/// let field = GrpcDescriptorReference {};
/// ```
/// A selected key and reference cannot overlap mutation of the owning entry.
/// ```compile_fail
/// use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;
/// fn overlap(prepared: &PreparedPluginReferences, owner: &mut edgion_resources::EdgionPlugins) {
///     let mut binding=prepared.bind_mut(owner).unwrap();
///     let mut entry=binding.request_entries().next().unwrap();
///     let refs=entry.references().unwrap();
///     let descriptor=refs.grpc_descriptor().unwrap().unwrap();
///     entry.config_mut();
///     descriptor.key();
/// }
/// ```
pub struct GrpcDescriptorReference<'a> {
    token: ReferenceToken<'a>,
    key: &'a str,
}
impl GrpcDescriptorReference<'_> {
    pub fn reference(&self) -> &ReferenceToken<'_> {
        &self.token
    }
    pub fn key(&self) -> &str {
        self.key
    }
}

// Only the actual owning entry can construct this pairing. In particular,
// OIDC/LDAP material lives outside their OutboundTlsLocal policy block.
pub(super) struct PluginTlsField<'a> {
    references: EntryReferences<'a>,
    policy: &'a OutboundTlsLocal,
    ca: Option<&'a [k8s_openapi::api::core::v1::Secret]>,
    client: Option<&'a k8s_openapi::api::core::v1::Secret>,
}
impl PluginTlsField<'_> {
    pub(super) fn validate_source(&self) -> Result<(), ReferenceError> {
        self.references.validate_source()
    }
    pub(super) fn policy(&self) -> &OutboundTlsLocal {
        self.policy
    }
    pub(super) fn ca_material(&self) -> Option<&[k8s_openapi::api::core::v1::Secret]> {
        self.ca
    }
    pub(super) fn client_material(&self) -> Option<&k8s_openapi::api::core::v1::Secret> {
        self.client
    }
    pub(super) fn visit_ca<'loan>(&'loan self, mut visit: impl FnMut(ReferenceToken<'loan>)) {
        for token in self.references.tokens(Role::Ca, None) {
            visit(token);
        }
    }
    pub(super) fn client_reference(&self) -> Option<ReferenceToken<'_>> {
        self.references.single(Role::Client)
    }
}
fn plugin_tls_field<'a>(
    references: EntryReferences<'a>,
    plugin: &'a EdgionPlugin,
) -> Result<super::outbound_tls::BoundOutboundTls<'a>, ReferenceError> {
    let (policy, ca, client) = tls_parts(plugin).ok_or(ReferenceError::StaleBinding)?;
    Ok(super::outbound_tls::BoundOutboundTls::from_plugin(PluginTlsField {
        references,
        policy,
        ca,
        client,
    }))
}
type TlsParts<'a> = (
    &'a OutboundTlsLocal,
    Option<&'a [k8s_openapi::api::core::v1::Secret]>,
    Option<&'a k8s_openapi::api::core::v1::Secret>,
);
fn switched_parts(tls: Option<&TlsConfig>) -> Option<TlsParts<'_>> {
    let tls = tls.filter(|tls| tls.enabled)?;
    Some((
        &tls.outbound,
        tls.resolved_ca_certificates.as_deref(),
        tls.resolved_client_certificate.as_ref(),
    ))
}
fn tls_parts(plugin: &EdgionPlugin) -> Option<TlsParts<'_>> {
    match plugin {
        EdgionPlugin::OpenidConnect(config) => Some((
            &config.tls,
            config.resolved_ca_certificates.as_deref(),
            config.resolved_client_certificate.as_ref(),
        )),
        EdgionPlugin::LdapAuth(config) => Some((
            &config.tls,
            config.resolved_ca_certificates.as_deref(),
            config.resolved_client_certificate.as_ref(),
        )),
        EdgionPlugin::ExtProc(config) => switched_parts(Some(&config.grpc_service.tls)),
        EdgionPlugin::Wasm(config) => switched_parts(config.source.fetch.as_ref().and_then(|fetch| fetch.tls.as_ref())),
        EdgionPlugin::ForwardAuth(config) => switched_parts(config.conn.tls.as_ref()),
        EdgionPlugin::Guardrail(config) => switched_parts(config.conn.tls.as_ref()),
        EdgionPlugin::AiGuard(config) => switched_parts(
            config
                .evaluator
                .as_ref()
                .and_then(|evaluator| evaluator.conn.tls.as_ref()),
        ),
        _ => None,
    }
}

impl EntryReferences<'_> {
    pub(super) fn validate_source(&self) -> Result<(), ReferenceError> {
        self.topology.owner.source().map(|_| ())
    }
    pub fn grpc_descriptor(&self) -> Option<Result<GrpcDescriptorReference<'_>, DescriptorReferenceError>> {
        if !self.prepared.supported || !self.prepared.enabled {
            return None;
        }
        let slot = self.prepared.atoms.iter().find_map(|atom| match atom {
            PreparedAtom::Reference {
                role: Role::Descriptor,
                slot,
                ..
            } => Some(slot),
            _ => None,
        })?;
        let token = slot.token(TopologyLoan::Plugins(self.topology));
        Some((|| {
            token.validated().map_err(DescriptorReferenceError::Reference)?;
            let FieldWire::Descriptor { key, .. } = &slot.wire else {
                return Err(DescriptorReferenceError::Reference(ReferenceError::StaleBinding));
            };
            if !crate::grpc_transcoding::valid_descriptor_key(key) {
                return Err(DescriptorReferenceError::InvalidSelector);
            }
            Ok(GrpcDescriptorReference { token, key })
        })())
    }
    pub fn request_secrets(&self) -> impl Iterator<Item = super::TemplateSecretReference<'_>> {
        self.prepared.atoms.iter().filter_map(move |atom| {
            if !self.prepared.supported || !self.prepared.enabled {
                return None;
            }
            match atom {
                PreparedAtom::Template(atom) => atom.reference(TopologyLoan::Plugins(self.topology)),
                _ => None,
            }
        })
    }
    fn tokens(&self, role: Role, group: Option<usize>) -> impl Iterator<Item = ReferenceToken<'_>> {
        self.prepared.atoms.iter().filter_map(move |atom| {
            if !self.prepared.supported || !self.prepared.enabled {
                return None;
            }
            if role == Role::HeaderCa
                && !self
                    .prepared
                    .atoms
                    .iter()
                    .any(|atom| matches!(atom, PreparedAtom::Presence(Role::HeaderCa, true)))
            {
                return None;
            }
            match atom {
                PreparedAtom::Reference {
                    role: current,
                    group: current_group,
                    slot,
                    ..
                } if *current == role && *current_group == group => {
                    Some(slot.token(TopologyLoan::Plugins(self.topology)))
                }
                _ => None,
            }
        })
    }
    fn single(&self, role: Role) -> Option<ReferenceToken<'_>> {
        self.tokens(role, None).next()
    }
    pub fn secret_groups(&self) -> impl Iterator<Item = SecretGroupReferences<'_>> {
        self.prepared.atoms.iter().filter_map(move |atom| match atom {
            PreparedAtom::Group { ordinal, name } if self.prepared.supported && self.prepared.enabled => {
                Some(SecretGroupReferences {
                    entry: self,
                    ordinal: *ordinal,
                    name,
                })
            }
            _ => None,
        })
    }
    pub fn header_ca_secrets(&self) -> impl Iterator<Item = ReferenceToken<'_>> {
        self.tokens(Role::HeaderCa, None)
    }
    pub fn csrf_secret(&self) -> Option<ReferenceToken<'_>> {
        self.single(Role::Csrf)
    }
    pub fn jwe_secret(&self) -> Option<ReferenceToken<'_>> {
        self.single(Role::Jwe)
    }
    pub fn oidc_client_secret(&self) -> Option<ReferenceToken<'_>> {
        self.single(Role::OidcClient)
    }
    pub fn oidc_session_secret(&self) -> Option<ReferenceToken<'_>> {
        self.single(Role::OidcSession)
    }
    pub fn ldap_bind_secret(&self) -> Option<ReferenceToken<'_>> {
        self.single(Role::LdapBind)
    }
    pub fn wasm_auth_header_secret(&self) -> Option<ReferenceToken<'_>> {
        self.single(Role::WasmAuth)
    }
    pub fn wasm_pull_secret(&self) -> Option<ReferenceToken<'_>> {
        self.single(Role::WasmPull)
    }
    pub fn credential(&self) -> Option<ReferenceToken<'_>> {
        self.single(Role::Credential)
    }
    pub fn tls(&self) -> Option<TlsReferences<'_>> {
        (self.prepared.supported
            && self.prepared.enabled
            && !self
                .prepared
                .atoms
                .iter()
                .any(|atom| matches!(atom, PreparedAtom::Presence(Role::TlsEnabled, false)))
            && self
                .prepared
                .atoms
                .iter()
                .any(|atom| matches!(atom, PreparedAtom::Presence(Role::Tls, true))))
        .then_some(TlsReferences { entry: self })
    }
}
impl SecretGroupReferences<'_> {
    pub fn name(&self) -> &str {
        self.name
    }
    pub fn references(&self) -> impl Iterator<Item = ReferenceToken<'_>> {
        self.entry.tokens(Role::Groups, Some(self.ordinal))
    }
}
impl TlsReferences<'_> {
    pub fn ca_certificates(&self) -> impl Iterator<Item = ReferenceToken<'_>> {
        self.entry.tokens(Role::Ca, None)
    }
    pub fn client_certificate(&self) -> Option<ReferenceToken<'_>> {
        self.entry.single(Role::Client)
    }
}

/// RequestMirror loans stay separate from credential topology and retain entry ordinals.
pub struct PluginMirrorRefs<'a> {
    owner: &'a EdgionPlugins,
}
impl<'a> PluginMirrorRefs<'a> {
    pub fn from_plugins(owner: &'a EdgionPlugins) -> Self {
        Self { owner }
    }
    pub fn authored_at(&self, index: usize) -> Option<super::route_fields::AuthoredMirrorField<'a>> {
        super::route_fields::plugin_mirror_field(self.owner, index)
    }
    pub fn authored(&self) -> impl Iterator<Item = super::route_fields::AuthoredMirrorField<'a>> + 'a {
        let owner = self.owner;
        (0..owner.spec.request_plugins.as_ref().map_or(0, Vec::len))
            .filter_map(move |index| super::route_fields::plugin_mirror_field(owner, index))
    }
    pub fn effective_at(&self, index: usize) -> Option<super::route_fields::EffectiveMirrorField<'a>> {
        self.authored_at(index).map(|field| field.field())
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/plugin_fields_test.rs"]
mod tests;
