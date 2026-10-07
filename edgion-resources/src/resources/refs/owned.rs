//! Private cold snapshots and short-lived capabilities minted by owning field visitors.
use super::link_sys_fields::LinkSysSnapshot;
use super::plugin_fields::PluginSnapshot;
use super::plugin_fields::{PluginStage, Role};
use super::{GrantQuery, ObjectIdentity, ReferenceError, ReferenceSource, ResourceScope};
use crate::resources::common::{CustomCACertificateRef, SecretObjectReference};
use kube::api::ObjectMeta;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// Fingerprint only the private reference descriptor, never a resource config
/// or resolved material. Stream serialization directly into the digest.
pub(super) fn topology_revision(value: &impl Serialize) -> String {
    struct DigestWriter<'a>(&'a mut Sha256);
    impl std::io::Write for DigestWriter<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut digest = Sha256::new();
    serde_json::to_writer(DigestWriter(&mut digest), value)
        .expect("reference descriptors have infallible serialization");
    format!("{:x}", digest.finalize())
}

#[derive(Clone, Copy)]
pub(super) enum TopologyLoan<'a> {
    Plugins(&'a Arc<PluginSnapshot>),
    LinkSys(&'a Arc<LinkSysSnapshot>),
    Conditions(&'a super::condition_fields::ConditionSnapshot),
    Egc(&'a super::egc_tls_fields::EgcTlsSnapshot),
}
impl<'a> TopologyLoan<'a> {
    fn owner(self) -> &'a OwnerSnapshot {
        match self {
            Self::Plugins(snapshot) => &snapshot.owner,
            Self::LinkSys(snapshot) => &snapshot.owner,
            Self::Conditions(snapshot) => &snapshot.owner,
            Self::Egc(snapshot) => &snapshot.owner,
        }
    }
    fn freeze(self) -> Result<FrozenTopology, ReferenceError> {
        match self {
            Self::Plugins(snapshot) => Ok(FrozenTopology::Plugins(Arc::clone(snapshot))),
            Self::LinkSys(snapshot) => Ok(FrozenTopology::LinkSys(Arc::clone(snapshot))),
            // Effective ConfigData-expanded conditions are synchronous loans.
            // They cannot be revalidated against an unexpanded raw-owner store.
            Self::Conditions(_) | Self::Egc(_) => Err(ReferenceError::UnsupportedTarget),
        }
    }
}

#[derive(Clone)]
enum FrozenTopology {
    Plugins(Arc<PluginSnapshot>),
    LinkSys(Arc<LinkSysSnapshot>),
}
impl FrozenTopology {
    fn owner(&self) -> &OwnerSnapshot {
        match self {
            Self::Plugins(snapshot) => &snapshot.owner,
            Self::LinkSys(snapshot) => &snapshot.owner,
        }
    }
    fn slot(&self, position: &FieldSlot) -> Option<&PreparedSlot> {
        match self {
            Self::Plugins(snapshot) => snapshot.slot(position),
            Self::LinkSys(snapshot) => snapshot.slot(position),
        }
    }
    fn revision(&self) -> &str {
        match self {
            Self::Plugins(snapshot) => &snapshot.revision,
            Self::LinkSys(snapshot) => &snapshot.revision,
        }
    }
    fn equivalent(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Plugins(left), Self::Plugins(right)) => Arc::ptr_eq(left, right) || left == right,
            (Self::LinkSys(left), Self::LinkSys(right)) => Arc::ptr_eq(left, right) || left == right,
            _ => false,
        }
    }
}

/// Immutable async descriptor, sharing one reference-only owning topology.
/// It confers no ongoing permission and never accepts a caller-supplied identity.
#[derive(Clone)]
pub struct FrozenFieldReference {
    topology: FrozenTopology,
    position: FieldSlot,
}
impl std::fmt::Debug for FrozenFieldReference {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("FrozenFieldReference").finish_non_exhaustive()
    }
}
impl PartialEq for FrozenFieldReference {
    fn eq(&self, other: &Self) -> bool {
        self.position == other.position && self.topology.equivalent(&other.topology)
    }
}
impl Eq for FrozenFieldReference {}
impl FrozenFieldReference {
    pub fn grant_query(&self) -> Result<GrantQuery<'_>, ReferenceError> {
        let slot = self.topology.slot(&self.position).ok_or(ReferenceError::StaleBinding)?;
        let target = slot.target.as_ref().map_err(|error| *error)?.borrowed()?;
        Ok(GrantQuery::new(self.topology.owner().source()?, target))
    }
    pub fn matches_plugins(&self, owner: &crate::resources::edgion_plugins::EdgionPlugins) -> bool {
        matches!(&self.topology, FrozenTopology::Plugins(snapshot) if snapshot.matches(owner))
            && self.grant_query().is_ok()
    }
    pub fn matches_link_sys(&self, owner: &crate::resources::link_sys::LinkSys) -> bool {
        matches!(&self.topology, FrozenTopology::LinkSys(snapshot) if snapshot.matches(owner))
            && self.grant_query().is_ok()
    }
    /// Allows one full current-owner comparison per shared snapshot in a batch.
    pub fn same_owner_snapshot(&self, other: &Self) -> bool {
        self.topology.equivalent(&other.topology)
    }
    /// Stable reference-only owner revision; combine it with the concrete slot
    /// and existing UID/RV/Grant inputs when computing an async work revision.
    pub fn owner_revision(&self) -> &str {
        self.topology.revision()
    }
    pub fn slot_revision(&self) -> String {
        topology_revision(&self.position)
    }
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub(super) struct OwnerSnapshot {
    group: &'static str,
    kind: &'static str,
    namespace: Option<String>,
    name: Option<String>,
    uid: Option<String>,
    generation: Option<i64>,
    cluster: bool,
}

impl OwnerSnapshot {
    pub(super) fn egc(metadata: &ObjectMeta) -> Self {
        let mut owner = Self::namespaced("edgion.io", "EdgionGatewayConfig", metadata);
        owner.cluster = true;
        owner
    }
    pub(super) fn namespaced(group: &'static str, kind: &'static str, metadata: &ObjectMeta) -> Self {
        Self {
            group,
            kind,
            namespace: metadata.namespace.clone(),
            name: metadata.name.clone(),
            uid: metadata.uid.clone(),
            generation: metadata.generation,
            cluster: false,
        }
    }
    pub(super) fn matches(&self, metadata: &ObjectMeta) -> bool {
        self.namespace == metadata.namespace
            && self.name == metadata.name
            && self.uid == metadata.uid
            && self.generation == metadata.generation
    }
    pub(super) fn namespace(&self) -> Option<&str> {
        if self.cluster {
            None
        } else {
            Some(self.namespace.as_deref().unwrap_or("default"))
        }
    }
    pub(super) fn source(&self) -> Result<ReferenceSource<'_>, ReferenceError> {
        if self.cluster && self.namespace.is_some() {
            return Err(ReferenceError::InvalidNamespace);
        }
        let scope = self
            .namespace()
            .map(ResourceScope::Namespaced)
            .unwrap_or(ResourceScope::Cluster);
        ObjectIdentity::checked(self.group, self.kind, scope, self.name.as_deref().unwrap_or_default())?;
        ReferenceSource::checked(self.group, self.kind, scope)
    }
    fn identity(&self) -> Result<ObjectIdentity<'_>, ReferenceError> {
        ObjectIdentity::checked(
            self.group,
            self.kind,
            self.namespace()
                .map(ResourceScope::Namespaced)
                .unwrap_or(ResourceScope::Cluster),
            self.name.as_deref().unwrap_or_default(),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FieldContract {
    CustomSecret,
    CustomCa,
    CustomCredential,
    TemplateSecret,
    FixedConfigMap,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(super) enum FieldWire {
    Secret(SecretObjectReference),
    Ca(CustomCACertificateRef),
    Template(super::template_fields::TemplateWire),
    Descriptor { name: String, key: String },
}

#[derive(Clone, PartialEq, Eq)]
enum TargetGrammar {
    Strict,
    Template,
}

#[derive(Clone, PartialEq, Eq)]
struct OwnedTarget {
    grammar: TargetGrammar,
    group: String,
    kind: String,
    namespace: String,
    name: String,
}

impl OwnedTarget {
    fn prepare(owner: &OwnerSnapshot, wire: &FieldWire, contract: FieldContract) -> Result<Self, ReferenceError> {
        if let FieldWire::Descriptor { name, .. } = wire {
            if contract != FieldContract::FixedConfigMap {
                return Err(ReferenceError::UnsupportedTarget);
            }
            let namespace = owner.namespace().ok_or(ReferenceError::MissingNamespace)?;
            ObjectIdentity::checked("", "ConfigMap", ResourceScope::Namespaced(namespace), name)?;
            owner.source()?;
            return Ok(Self {
                grammar: TargetGrammar::Strict,
                group: String::new(),
                kind: "ConfigMap".into(),
                namespace: namespace.into(),
                name: name.clone(),
            });
        }
        if let FieldWire::Template(wire) = wire {
            if contract != FieldContract::TemplateSecret {
                return Err(ReferenceError::UnsupportedTarget);
            }
            let (namespace, name) = wire.target().ok_or(ReferenceError::InvalidName)?;
            ObjectIdentity::parsed_template_secret(namespace, name)?;
            owner.source()?;
            return Ok(Self {
                grammar: TargetGrammar::Template,
                group: String::new(),
                kind: "Secret".to_string(),
                namespace: namespace.to_string(),
                name: name.to_string(),
            });
        }
        let (group, kind, namespace, name) = match wire {
            FieldWire::Secret(reference) => (
                reference.group.as_deref().unwrap_or(""),
                reference.kind.as_deref().unwrap_or("Secret"),
                reference.namespace.as_deref(),
                reference.name.as_str(),
            ),
            FieldWire::Ca(reference) => (
                reference.group.as_deref().unwrap_or(""),
                reference.kind.as_str(),
                reference.namespace.as_deref(),
                reference.name.as_str(),
            ),
            FieldWire::Template(_) | FieldWire::Descriptor { .. } => return Err(ReferenceError::UnsupportedTarget),
        };
        let namespace = namespace
            .or(owner.namespace())
            .ok_or(ReferenceError::MissingNamespace)?;
        // Only these private contracts are selected by the finite owning visitors.
        // CredentialInjector's LinkSys branch remains an exact non-core target.
        let group = if group == "core" && matches!(kind, "Secret" | "ConfigMap") {
            ""
        } else {
            group
        };
        ObjectIdentity::checked(group, kind, ResourceScope::Namespaced(namespace), name)?;
        let supported = match contract {
            FieldContract::CustomSecret => group.is_empty() && kind == "Secret",
            FieldContract::CustomCa => group.is_empty() && matches!(kind, "Secret" | "ConfigMap"),
            FieldContract::CustomCredential => {
                (group.is_empty() && kind == "Secret") || (group == "edgion.io" && kind == "LinkSys")
            }
            FieldContract::TemplateSecret | FieldContract::FixedConfigMap => false,
        };
        if !supported {
            return Err(ReferenceError::UnsupportedTarget);
        }
        owner.source()?;
        Ok(Self {
            grammar: TargetGrammar::Strict,
            group: group.to_string(),
            kind: kind.to_string(),
            namespace: namespace.to_string(),
            name: name.to_string(),
        })
    }
    fn borrowed(&self) -> Result<ObjectIdentity<'_>, ReferenceError> {
        if self.grammar == TargetGrammar::Template {
            return ObjectIdentity::parsed_template_secret(&self.namespace, &self.name);
        }
        ObjectIdentity::checked(
            &self.group,
            &self.kind,
            ResourceScope::Namespaced(&self.namespace),
            &self.name,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(super) enum FieldSlot {
    EgcTls {
        ca: Option<usize>,
    },
    PluginTemplate {
        stage: PluginStage,
        entry: usize,
        position: crate::resources::common::outbound_request::RequestTemplatePosition,
        segment: usize,
    },
    LinkSysTemplate {
        provider: super::link_sys_fields::Provider,
        position: crate::resources::common::outbound_request::RequestTemplatePosition,
        segment: usize,
    },
    Condition {
        root: usize,
        leaf: super::condition_fields::LeafPosition,
        group: Option<usize>,
        reference: usize,
    },
    Plugin {
        stage: PluginStage,
        entry: usize,
        role: Role,
        group: Option<usize>,
        reference: usize,
    },
    LinkSys {
        provider: super::link_sys_fields::Provider,
        role: super::link_sys_fields::Role,
        reference: usize,
    },
}

impl FieldSlot {
    fn matches_plugin(
        &self,
        stage: PluginStage,
        entry: usize,
        role: Role,
        group: Option<usize>,
        reference: usize,
    ) -> bool {
        match self {
            Self::Plugin {
                stage: current_stage,
                entry: current_entry,
                role: current_role,
                group: current_group,
                reference: current_reference,
            } => {
                *current_stage == stage
                    && *current_entry == entry
                    && *current_role == role
                    && *current_group == group
                    && *current_reference == reference
            }
            Self::LinkSys { .. }
            | Self::Condition { .. }
            | Self::PluginTemplate { .. }
            | Self::LinkSysTemplate { .. }
            | Self::EgcTls { .. } => false,
        }
    }
}

#[derive(PartialEq, Eq, Serialize)]
pub(super) struct PreparedSlot {
    pub(super) wire: FieldWire,
    position: FieldSlot,
    #[serde(skip)]
    target: Result<OwnedTarget, ReferenceError>,
}

impl PreparedSlot {
    pub(super) fn new(owner: &OwnerSnapshot, wire: FieldWire, contract: FieldContract, position: FieldSlot) -> Self {
        let target = OwnedTarget::prepare(owner, &wire, contract);
        Self { wire, target, position }
    }
    pub(super) fn matches_plugin(
        &self,
        stage: PluginStage,
        entry: usize,
        role: Role,
        group: Option<usize>,
        reference: usize,
    ) -> bool {
        self.position.matches_plugin(stage, entry, role, group, reference)
    }
    pub(super) fn token<'a>(&'a self, topology: TopologyLoan<'a>) -> ReferenceToken<'a> {
        ReferenceToken {
            slot: self,
            owner: topology.owner(),
            topology,
        }
    }
    pub(super) fn position(&self) -> &FieldSlot {
        &self.position
    }
    pub(super) fn matches_link_sys(
        &self,
        provider: super::link_sys_fields::Provider,
        role: super::link_sys_fields::Role,
        reference: usize,
    ) -> bool {
        matches!(&self.position, FieldSlot::LinkSys { provider: current_provider, role: current_role, reference: current_reference }
            if *current_provider == provider && *current_role == role && *current_reference == reference)
    }
    pub(super) fn unsupported_target(&self) -> bool {
        matches!(self.target, Err(ReferenceError::UnsupportedTarget))
    }
}

/// An opaque capability loan from an actual bound resource field.
/// Constructors are private; a caller cannot supply its own source/profile/slot.
#[derive(Clone, Copy)]
pub struct ReferenceToken<'a> {
    slot: &'a PreparedSlot,
    owner: &'a OwnerSnapshot,
    topology: TopologyLoan<'a>,
}

impl ReferenceToken<'_> {
    /// A managed CredentialInjector output is local to its actual plugin owner.
    /// It is derived only from the LinkSys branch of that specific field.
    pub fn managed_credential_output(&self) -> Result<BoundFieldReference<'_>, ReferenceError> {
        let reference = self.validated()?;
        if !matches!(
            self.slot.position,
            FieldSlot::Plugin {
                role: Role::Credential,
                ..
            }
        ) || reference.target().group_kind().group() != "edgion.io"
            || reference.target().group_kind().kind() != "LinkSys"
        {
            return Err(ReferenceError::UnsupportedTarget);
        }
        Ok(BoundFieldReference {
            source: reference.source(),
            owner: reference.owner(),
            target: ObjectIdentity::checked("", "Secret", reference.owner().scope(), reference.owner().name())?,
            _position: &self.slot.position,
        })
    }
    /// Freeze only a validated descriptor for async work. Runtime consumers
    /// must recheck its real owner, registry and current Grant before use.
    pub fn freeze(&self) -> Result<FrozenFieldReference, ReferenceError> {
        self.validated()?;
        Ok(FrozenFieldReference {
            topology: self.topology.freeze()?,
            position: self.slot.position.clone(),
        })
    }
    /// Validate one slot without suppressing its valid sibling slots.
    pub fn validated(&self) -> Result<BoundFieldReference<'_>, ReferenceError> {
        let target = self.slot.target.as_ref().map_err(|error| *error)?.borrowed()?;
        Ok(BoundFieldReference {
            source: self.owner.source()?,
            owner: self.owner.identity()?,
            target,
            _position: &self.slot.position,
        })
    }
    pub fn authored_name(&self) -> &str {
        match &self.slot.wire {
            FieldWire::Secret(reference) => &reference.name,
            FieldWire::Ca(reference) => &reference.name,
            FieldWire::Template(reference) => reference.target().map_or("", |(_, name)| name),
            FieldWire::Descriptor { name, .. } => name,
        }
    }
}

/// A validated source and concrete target retaining the active owning-field loan.
/// It contains no Secret material, runtime store, policy switch or operation handle.
#[derive(Clone, Copy, Debug)]
pub struct BoundFieldReference<'a> {
    source: ReferenceSource<'a>,
    owner: ObjectIdentity<'a>,
    target: ObjectIdentity<'a>,
    _position: &'a FieldSlot,
}

impl<'a> BoundFieldReference<'a> {
    pub fn source(self) -> ReferenceSource<'a> {
        self.source
    }
    pub fn owner(self) -> ObjectIdentity<'a> {
        self.owner
    }
    pub fn target(self) -> ObjectIdentity<'a> {
        self.target
    }
    pub fn grant_query(self) -> GrantQuery<'a> {
        GrantQuery::new(self.source, self.target)
    }
    pub fn is_same_namespace(self) -> bool {
        matches!((self.source.scope(), self.target.scope()), (ResourceScope::Namespaced(source), ResourceScope::Namespaced(target)) if source == target)
    }
}
