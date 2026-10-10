//! Private cold snapshots and short-lived capabilities minted by owning field visitors.
use super::link_sys_fields::LinkSysSnapshot;
use super::plugin_fields::PluginSnapshot;
use super::plugin_fields::{PluginStage, Role};
use super::{GrantQuery, ObjectIdentity, ReferenceError, ReferenceSource, ResourceScope};
use crate::resources::common::{is_core_api_group, CustomCACertificateRef, SecretObjectReference};
use crate::EdgionBackend;
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
    IpList(&'a super::ip_list_fields::IpListSnapshot),
    Egc(&'a super::egc_tls_fields::EgcTlsSnapshot),
    RouteAuth(&'a super::route_auth_fields::RouteAuthSnapshot),
    GatewayTls(&'a OwnerSnapshot),
    BackendTls(&'a OwnerSnapshot),
    HealthTls(&'a OwnerSnapshot),
    EdgionTls(&'a OwnerSnapshot),
    Acme(&'a OwnerSnapshot),
}
impl<'a> TopologyLoan<'a> {
    fn owner(self) -> &'a OwnerSnapshot {
        match self {
            Self::Plugins(snapshot) => &snapshot.owner,
            Self::LinkSys(snapshot) => &snapshot.owner,
            Self::Conditions(snapshot) => &snapshot.owner,
            Self::IpList(snapshot) => &snapshot.owner,
            Self::Egc(snapshot) => &snapshot.owner,
            Self::RouteAuth(snapshot) => &snapshot.owner,
            Self::GatewayTls(owner)
            | Self::BackendTls(owner)
            | Self::HealthTls(owner)
            | Self::EdgionTls(owner)
            | Self::Acme(owner) => owner,
        }
    }
    fn freeze(self) -> Result<FrozenTopology, ReferenceError> {
        match self {
            Self::Plugins(snapshot) => Ok(FrozenTopology::Plugins(Arc::clone(snapshot))),
            Self::LinkSys(snapshot) => Ok(FrozenTopology::LinkSys(Arc::clone(snapshot))),
            // Effective ConfigData-expanded conditions are synchronous loans.
            // They cannot be revalidated against an unexpanded raw-owner store.
            Self::Conditions(_)
            | Self::IpList(_)
            | Self::Egc(_)
            | Self::RouteAuth(_)
            | Self::GatewayTls(_)
            | Self::BackendTls(_)
            | Self::HealthTls(_)
            | Self::EdgionTls(_)
            | Self::Acme(_) => Err(ReferenceError::UnsupportedTarget),
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
    pub fn managed_credential_output(&self) -> Result<BoundFieldReference<'_>, ReferenceError> {
        let slot = self.topology.slot(&self.position).ok_or(ReferenceError::StaleBinding)?;
        managed_credential_output(self.topology.owner(), slot)
    }
    pub fn validate_managed_credential_secret(
        &self,
        secret: &k8s_openapi::api::core::v1::Secret,
    ) -> Result<(), ReferenceError> {
        validate_secret_metadata(self.managed_credential_output()?, secret)
    }

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
    StrictSecret,
    NativeSecret,
    NativeCa,
    CustomSecret,
    CustomCa,
    CustomCredential,
    TemplateSecret,
    FixedConfigMap,
    FixedConfigData,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(super) enum FieldWire {
    Secret(SecretObjectReference),
    Ca(CustomCACertificateRef),
    NativeCa(crate::resources::gateway::FrontendCACertificateRef),
    BackendCa(crate::resources::backend_tls_policy::BackendTLSPolicyCACertificateRef),
    Template(super::template_fields::TemplateWire),
    Descriptor { name: String, key: String },
    ConfigData { name: String, namespace: Option<String> },
}

#[derive(Clone, PartialEq, Eq)]
enum TargetGrammar {
    Strict,
    NativeObject,
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
        // Preserve target-before-source error precedence for existing fields.
        let target = Self::prepare_in_namespace(owner.namespace(), wire, contract)?;
        owner.source()?;
        Ok(target)
    }
    fn prepare_in_namespace(
        default_namespace: Option<&str>,
        wire: &FieldWire,
        contract: FieldContract,
    ) -> Result<Self, ReferenceError> {
        if let FieldWire::Descriptor { name, .. } = wire {
            if contract != FieldContract::FixedConfigMap {
                return Err(ReferenceError::UnsupportedTarget);
            }
            let namespace = default_namespace.ok_or(ReferenceError::MissingNamespace)?;
            ObjectIdentity::checked("", "ConfigMap", ResourceScope::Namespaced(namespace), name)?;
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
            return Ok(Self {
                grammar: TargetGrammar::Template,
                group: String::new(),
                kind: "Secret".to_string(),
                namespace: namespace.to_string(),
                name: name.to_string(),
            });
        }
        let (group, kind, namespace, name) = match wire {
            FieldWire::ConfigData { name, namespace } => {
                ("edgion.io", "EdgionConfigData", namespace.as_deref(), name.as_str())
            }
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
            FieldWire::NativeCa(reference) => (
                reference.group.as_str(),
                reference.kind.as_str(),
                reference.namespace.as_deref(),
                reference.name.as_str(),
            ),
            FieldWire::BackendCa(reference) => (
                reference.group.as_str(),
                reference.kind.as_str(),
                None,
                reference.name.as_str(),
            ),
            FieldWire::Template(_) | FieldWire::Descriptor { .. } => return Err(ReferenceError::UnsupportedTarget),
        };
        let namespace = namespace
            .or(default_namespace)
            .ok_or(ReferenceError::MissingNamespace)?;
        // Only these private contracts are selected by the finite owning visitors.
        // CredentialInjector's LinkSys branch remains an exact non-core target.
        let native = matches!(contract, FieldContract::NativeSecret | FieldContract::NativeCa);
        let group = if !native
            && contract != FieldContract::StrictSecret
            && group == "core"
            && matches!(kind, "Secret" | "ConfigMap")
        {
            ""
        } else {
            group
        };
        if native {
            ObjectIdentity::native_object_reference(group, kind, ResourceScope::Namespaced(namespace), name)?;
        } else {
            ObjectIdentity::checked(group, kind, ResourceScope::Namespaced(namespace), name)?;
        }
        let supported = match contract {
            FieldContract::NativeSecret | FieldContract::StrictSecret => group.is_empty() && kind == "Secret",
            FieldContract::NativeCa => group.is_empty() && matches!(kind, "Secret" | "ConfigMap"),
            FieldContract::FixedConfigData => group == "edgion.io" && kind == "EdgionConfigData",
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
        Ok(Self {
            grammar: if native {
                TargetGrammar::NativeObject
            } else {
                TargetGrammar::Strict
            },
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
        if self.grammar == TargetGrammar::NativeObject {
            return ObjectIdentity::native_object_reference(
                &self.group,
                &self.kind,
                ResourceScope::Namespaced(&self.namespace),
                &self.name,
            );
        }
        ObjectIdentity::checked(
            &self.group,
            &self.kind,
            ResourceScope::Namespaced(&self.namespace),
            &self.name,
        )
    }
}

/// Effective TLS target identity without an authored owner, position or token.
pub(super) struct PreparedTlsTarget {
    target: Result<OwnedTarget, ReferenceError>,
}
impl PreparedTlsTarget {
    pub(super) fn ca(namespace: &str, wire: &CustomCACertificateRef) -> Self {
        Self {
            target: OwnedTarget::prepare_in_namespace(
                Some(namespace),
                &FieldWire::Ca(wire.clone()),
                FieldContract::CustomCa,
            ),
        }
    }
    pub(super) fn client(namespace: &str, wire: &SecretObjectReference) -> Self {
        Self {
            target: OwnedTarget::prepare_in_namespace(
                Some(namespace),
                &FieldWire::Secret(wire.clone()),
                FieldContract::CustomSecret,
            ),
        }
    }
    pub(super) fn target(&self) -> Result<ObjectIdentity<'_>, ReferenceError> {
        self.target.as_ref().map_err(|error| *error)?.borrowed()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(super) enum FieldSlot {
    AcmeAccount,
    AcmeEab,
    EdgionTlsServer,
    EdgionTlsClientCa,
    HealthTls {
        ca: Option<usize>,
    },
    BackendTlsCa {
        reference: usize,
    },
    BackendTlsClient,
    GatewayBackendTls,
    GatewayServing {
        listener: usize,
        reference: usize,
    },
    GatewayFrontendCa {
        per_port: Option<usize>,
        reference: usize,
    },
    RouteAuthTls {
        rule: usize,
        backend: Option<usize>,
        filter: usize,
        ca: Option<usize>,
    },
    RouteAuthTemplate {
        rule: usize,
        backend: Option<usize>,
        filter: usize,
        position: crate::resources::common::outbound_request::RequestTemplatePosition,
        segment: usize,
    },
    BackendCredential {
        index: usize,
    },
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
    IpList {
        root: usize,
        reference: usize,
        deny: bool,
    },
    ConditionIp {
        root: usize,
        leaf: Option<super::condition_fields::LeafPosition>,
        reference: usize,
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
            Self::AcmeAccount
            | Self::AcmeEab
            | Self::EdgionTlsServer
            | Self::EdgionTlsClientCa
            | Self::HealthTls { .. }
            | Self::BackendTlsCa { .. }
            | Self::BackendTlsClient
            | Self::GatewayBackendTls
            | Self::GatewayServing { .. }
            | Self::GatewayFrontendCa { .. }
            | Self::RouteAuthTls { .. }
            | Self::RouteAuthTemplate { .. }
            | Self::BackendCredential { .. }
            | Self::LinkSys { .. }
            | Self::IpList { .. }
            | Self::ConditionIp { .. }
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

fn managed_credential_output<'a>(
    owner: &'a OwnerSnapshot,
    slot: &'a PreparedSlot,
) -> Result<BoundFieldReference<'a>, ReferenceError> {
    let target = slot.target.as_ref().map_err(|error| *error)?.borrowed()?;
    if !matches!(
        slot.position,
        FieldSlot::Plugin {
            role: Role::Credential,
            ..
        }
    ) || target.group_kind().group() != "edgion.io"
        || target.group_kind().kind() != "LinkSys"
    {
        return Err(ReferenceError::UnsupportedTarget);
    }
    let identity = owner.identity()?;
    Ok(BoundFieldReference {
        source: owner.source()?,
        owner: identity,
        target: ObjectIdentity::checked("", "Secret", identity.scope(), identity.name())?,
        _position: &slot.position,
    })
}
pub(super) fn validate_secret_metadata(
    reference: BoundFieldReference<'_>,
    secret: &k8s_openapi::api::core::v1::Secret,
) -> Result<(), ReferenceError> {
    let target = reference.target();
    if secret.metadata.name.as_deref() != Some(target.name())
        || secret.metadata.namespace.as_deref() != target.scope().namespace()
    {
        return Err(ReferenceError::StaleBinding);
    }
    Ok(())
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
        managed_credential_output(self.owner, self.slot)
    }
    pub fn validate_managed_credential_secret(
        &self,
        secret: &k8s_openapi::api::core::v1::Secret,
    ) -> Result<(), ReferenceError> {
        validate_secret_metadata(self.managed_credential_output()?, secret)
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
            FieldWire::NativeCa(reference) => &reference.name,
            FieldWire::BackendCa(reference) => &reference.name,
            FieldWire::Template(reference) => reference.target().map_or("", |(_, name)| name),
            FieldWire::Descriptor { name, .. } | FieldWire::ConfigData { name, .. } => name,
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
    pub(super) fn backend_secret(backend: &'a EdgionBackend, position: &'a FieldSlot) -> Result<Self, ReferenceError> {
        let FieldSlot::BackendCredential { index } = position else {
            return Err(ReferenceError::StaleBinding);
        };
        let namespace = backend.metadata.namespace.as_deref().unwrap_or("default");
        let scope = ResourceScope::Namespaced(namespace);
        let owner = ObjectIdentity::checked(
            "edgion.io",
            "EdgionBackend",
            scope,
            backend.metadata.name.as_deref().unwrap_or_default(),
        )?;
        let source = ReferenceSource::checked("edgion.io", "EdgionBackend", scope)?;
        let wire = &backend
            .spec
            .ai
            .credential_pool
            .credentials
            .get(*index)
            .ok_or(ReferenceError::StaleBinding)?
            .credential
            .secret_ref;
        if !is_core_api_group(wire.group.as_deref().unwrap_or_default())
            || wire.kind.as_deref().unwrap_or("Secret") != "Secret"
        {
            return Err(ReferenceError::UnsupportedTarget);
        }
        let target = ObjectIdentity::checked(
            "",
            "Secret",
            ResourceScope::Namespaced(wire.namespace.as_deref().unwrap_or(namespace)),
            &wire.name,
        )?;
        Ok(Self {
            source,
            owner,
            target,
            _position: position,
        })
    }

    /// Finite native Gateway TLS fields; no arbitrary caller can mint this position.
    pub fn is_gateway_tls(self) -> bool {
        matches!(
            self._position,
            FieldSlot::GatewayBackendTls | FieldSlot::GatewayServing { .. } | FieldSlot::GatewayFrontendCa { .. }
        )
    }
    pub(super) fn is_gateway_backend_tls_material(self) -> bool {
        matches!(self._position, FieldSlot::GatewayBackendTls)
    }
    /// Actual BackendTLSPolicy material fields, never an arbitrary caller profile.
    pub fn is_backend_tls_material(self) -> bool {
        matches!(
            self._position,
            FieldSlot::BackendTlsCa { .. } | FieldSlot::BackendTlsClient
        )
    }
    pub fn is_acme_account(self) -> bool {
        matches!(self._position, FieldSlot::AcmeAccount)
    }
    pub fn is_edgion_tls_material(self) -> bool {
        matches!(
            self._position,
            FieldSlot::EdgionTlsServer | FieldSlot::EdgionTlsClientCa
        )
    }
    pub fn is_health_tls_material(self) -> bool {
        matches!(self._position, FieldSlot::HealthTls { .. })
    }
    pub fn is_gateway_frontend_ca(self) -> bool {
        matches!(self._position, FieldSlot::GatewayFrontendCa { .. })
    }
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
