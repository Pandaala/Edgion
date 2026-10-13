//! Short-lived owning references in effective EdgionPlugins condition trees.
//!
//! Prepare after ConfigData expansion. Effective sidecars are deliberately
//! separate from the raw-owner topology used by asynchronous frozen references.
use super::owned::{FieldContract, FieldSlot, FieldWire, OwnerSnapshot, PreparedSlot, TopologyLoan};
use super::plugin_fields::{supports_stage, PluginStage};
use super::{ReferenceError, ReferenceToken};
use crate::resources::edgion_config_data::EdgionConfigDataRef;
use crate::resources::edgion_plugins::conditions::{AnyOfClause, Condition, ConditionSet, PluginConditions};
use crate::resources::edgion_plugins::dye::DyeSuite;
use crate::resources::edgion_plugins::ip_reference_set::{IpReadDomain, IpReadResolution, IpReadScope, IpReferenceSet};
use crate::resources::edgion_plugins::plugin_configs::RequestAccessRuleType;
use crate::resources::edgion_plugins::{EdgionPlugin, EdgionPlugins};
use crate::EdgionGatewayConfig;
use serde::Serialize;
use std::mem::{discriminant, Discriminant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(super) enum LeafPosition {
    AllOf(usize),
    AnyOf(usize),
    #[serde(rename = "AnyOfAllOf")]
    AnyClauseAll {
        clause: usize,
        leaf: usize,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Direct,
    Skip,
    Run,
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum Location {
    RealIp,
    Entry {
        present: bool,
    },
    Body {
        present: bool,
        conditions: bool,
    },
    Dye {
        response: bool,
        rule: usize,
        conditions: bool,
    },
    Restriction {
        deny: bool,
    },
    RapIp {
        profile: String,
        group: usize,
        rule: usize,
        direct: bool,
    },
    RapUrl {
        profile: String,
        group: usize,
        rule: usize,
        candidate: usize,
    },
    Boundary {
        tag: &'static str,
        key: Option<String>,
        first: usize,
        second: usize,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct RootPosition {
    stage: PluginStage,
    entry: usize,
    location: Location,
    action: Action,
}
struct Root<'a> {
    position: RootPosition,
    enabled: bool,
    supported: bool,
    variant: &'static str,
    conditions: Option<&'a ConditionSet>,
}
#[derive(PartialEq, Eq)]
enum Shape {
    AllOf(usize),
    AnyOf(Vec<ClauseShape>),
}
#[derive(PartialEq, Eq)]
enum ClauseShape {
    Direct,
    AllOf { name: Option<String>, length: usize },
}
struct PreparedGroup {
    name: String,
    slots: Vec<PreparedSlot>,
}
struct PreparedLeaf {
    position: LeafPosition,
    variant: Discriminant<Condition>,
    key: Option<String>,
    secret: Option<PreparedSlot>,
    groups: Option<Vec<PreparedGroup>>,
    ip: Option<PreparedIp>,
}
struct PreparedSet {
    shape: Shape,
    leaves: Vec<PreparedLeaf>,
}
struct PreparedRoot {
    position: RootPosition,
    enabled: bool,
    supported: bool,
    variant: &'static str,
    set: Option<PreparedSet>,
    ip: Option<PreparedIp>,
}
pub(super) struct ConditionSnapshot {
    global: bool,
    pub(super) owner: OwnerSnapshot,
    stages: [Option<usize>; 4],
    roots: Vec<PreparedRoot>,
}

/// No constructor accepts a borrowed leaf, position, source or profile.
///
/// A condition token cannot escape its callback.
/// ```compile_fail
/// use edgion_resources::resources::refs::condition_fields::PreparedConditionReferences;
/// let ep: edgion_resources::EdgionPlugins = serde_json::from_value(serde_json::json!({
///     "metadata":{"name":"plugins","namespace":"default"}, "spec":{}
/// })).unwrap();
/// let prepared = PreparedConditionReferences::from_plugins(&ep);
/// let mut escaped = None;
/// prepared.visit(&ep, |leaf| {
///     let references = leaf.references().unwrap();
///     escaped = references.secret_match_secret();
/// }).unwrap();
/// let _ = escaped.unwrap().validated();
/// ```
/// A mutable leaf cannot be changed while its reference loan is live.
/// ```compile_fail
/// use edgion_resources::resources::refs::condition_fields::PreparedConditionReferences;
/// let mut ep: edgion_resources::EdgionPlugins = serde_json::from_value(serde_json::json!({
///     "metadata":{"name":"plugins","namespace":"default"}, "spec":{}
/// })).unwrap();
/// let prepared = PreparedConditionReferences::from_plugins(&ep);
/// prepared.visit_mut(&mut ep, |mut leaf| {
///     let references = leaf.references().unwrap();
///     let token = references.secret_match_secret().unwrap();
///     let _ = leaf.config_mut();
///     let _ = token.validated();
/// }).unwrap();
/// ```
pub struct PreparedConditionReferences {
    snapshot: ConditionSnapshot,
}

impl PreparedConditionReferences {
    pub fn from_plugins(owner: &EdgionPlugins) -> Self {
        let source = OwnerSnapshot::namespaced("edgion.io", "EdgionPlugins", &owner.metadata);
        let roots = roots(owner)
            .into_iter()
            .enumerate()
            .map(|(ordinal, root)| {
                let set = root.conditions.map(|set| PreparedSet::new(&source, ordinal, set));
                PreparedRoot {
                    position: root.position.clone(),
                    enabled: root.enabled,
                    supported: root.supported,
                    variant: root.variant,
                    set,
                    ip: direct_ip(owner, &root.position).map(|ip| PreparedIp::new(&source, ordinal, None, ip)),
                }
            })
            .collect();
        Self {
            snapshot: ConditionSnapshot {
                global: false,
                owner: source,
                stages: stage_shape(owner),
                roots,
            },
        }
    }
    pub fn from_gateway_config(owner: &EdgionGatewayConfig) -> Self {
        let source = OwnerSnapshot::egc(&owner.metadata);
        let roots = owner
            .spec
            .real_ip
            .as_ref()
            .map(|config| PreparedRoot {
                position: RootPosition {
                    stage: PluginStage::Request,
                    entry: 0,
                    location: Location::RealIp,
                    action: Action::Direct,
                },
                enabled: true,
                supported: true,
                variant: "RealIp",
                set: None,
                ip: Some(PreparedIp::new(&source, 0, None, &config.trusted)),
            })
            .into_iter()
            .collect();
        Self {
            snapshot: ConditionSnapshot {
                global: true,
                owner: source,
                stages: [None; 4],
                roots,
            },
        }
    }
    fn check_gateway(&self, owner: &EdgionGatewayConfig) -> Result<(), ReferenceError> {
        if !self.snapshot.global || !self.snapshot.owner.matches(&owner.metadata) {
            return Err(ReferenceError::StaleBinding);
        }
        match (owner.spec.real_ip.as_ref(), self.snapshot.roots.as_slice()) {
            (None, []) => Ok(()),
            (Some(config), [root]) if root.ip.as_ref().is_some_and(|ip| ip.matches(&config.trusted)) => Ok(()),
            _ => Err(ReferenceError::StaleBinding),
        }
    }
    pub fn visit_gateway_ip(
        &self,
        owner: &EdgionGatewayConfig,
        mut visit: impl for<'leaf> FnMut(BoundIpAccess<'leaf>),
    ) -> Result<(), ReferenceError> {
        self.check_gateway(owner)?;
        self.snapshot.owner.source()?;
        if let Some(config) = owner.spec.real_ip.as_ref() {
            visit(BoundIpAccess {
                config: &config.trusted,
                prepared: self.snapshot.roots[0].ip.as_ref().ok_or(ReferenceError::StaleBinding)?,
                snapshot: &self.snapshot,
                consumer: IpReferenceConsumer::RealIp,
                role: IpExecutionRole::Trust,
                supported: true,
            });
        }
        Ok(())
    }
    pub fn visit_gateway_ip_mut(
        &self,
        owner: &mut EdgionGatewayConfig,
        mut visit: impl for<'leaf> FnMut(BoundIpAccessMut<'leaf>),
    ) -> Result<(), ReferenceError> {
        self.check_gateway(owner)?;
        self.snapshot.owner.source()?;
        if let Some(config) = owner.spec.real_ip.as_mut() {
            visit(BoundIpAccessMut {
                config: &mut config.trusted,
                prepared: self.snapshot.roots[0].ip.as_ref().ok_or(ReferenceError::StaleBinding)?,
                snapshot: &self.snapshot,
                consumer: IpReferenceConsumer::RealIp,
                role: IpExecutionRole::Trust,
                supported: true,
            });
        }
        Ok(())
    }
    pub fn clear_gateway_ip_scopes(&self, owner: &mut EdgionGatewayConfig) -> Result<(), ReferenceError> {
        self.check_gateway(owner)?;
        if let Some(config) = owner.spec.real_ip.as_mut() {
            config.trusted.resolved_refs = None;
            for reference in &mut config.trusted.ip_refs {
                reference.ref_denied = None;
            }
        }
        Ok(())
    }
    pub fn visit(
        &self,
        owner: &EdgionPlugins,
        mut visit: impl for<'leaf> FnMut(BoundCondition<'leaf>),
    ) -> Result<(), ReferenceError> {
        self.check(owner)?;
        for (root, prepared) in roots(owner).into_iter().zip(&self.snapshot.roots) {
            let (Some(set), Some(prepared_set)) = (root.conditions, prepared.set.as_ref()) else {
                continue;
            };
            for leaf in &prepared_set.leaves {
                let config = leaf_at(set, leaf.position).ok_or(ReferenceError::StaleBinding)?;
                visit(BoundCondition {
                    config,
                    prepared: leaf,
                    snapshot: &self.snapshot,
                    active: prepared.enabled && prepared.supported,
                });
            }
        }
        Ok(())
    }
    pub fn visit_mut(
        &self,
        owner: &mut EdgionPlugins,
        mut visit: impl for<'leaf> FnMut(BoundConditionMut<'leaf>),
    ) -> Result<(), ReferenceError> {
        self.check(owner)?;
        for root in &self.snapshot.roots {
            let Some(prepared_set) = root.set.as_ref() else {
                continue;
            };
            let set = root_mut(owner, &root.position).ok_or(ReferenceError::StaleBinding)?;
            for leaf in &prepared_set.leaves {
                let config = leaf_at_mut(set, leaf.position).ok_or(ReferenceError::StaleBinding)?;
                visit(BoundConditionMut {
                    config,
                    prepared: leaf,
                    snapshot: &self.snapshot,
                    active: root.enabled && root.supported,
                });
            }
        }
        Ok(())
    }
    fn check(&self, owner: &EdgionPlugins) -> Result<(), ReferenceError> {
        if self.snapshot.global
            || !self.snapshot.owner.matches(&owner.metadata)
            || self.snapshot.stages != stage_shape(owner)
        {
            return Err(ReferenceError::StaleBinding);
        }
        let actual = roots(owner);
        if actual.len() != self.snapshot.roots.len()
            || !actual.iter().zip(&self.snapshot.roots).all(|(root, prepared)| {
                root.position == prepared.position
                    && root.enabled == prepared.enabled
                    && root.supported == prepared.supported
                    && root.variant == prepared.variant
                    && match (direct_ip(owner, &root.position), &prepared.ip) {
                        (None, None) => true,
                        (Some(ip), Some(prepared)) => prepared.matches(ip),
                        _ => false,
                    }
                    && match (root.conditions, &prepared.set) {
                        (None, None) => true,
                        (Some(set), Some(prepared)) => prepared.matches(set),
                        _ => false,
                    }
            })
        {
            return Err(ReferenceError::StaleBinding);
        }
        Ok(())
    }
}

/// IP consumers retain their established resolution policy independently of credentials.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpExecutionRole {
    Required,
    Candidate,
    Observational,
    Partial,
    Trust,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpReferenceConsumer {
    RealIp,
    Ordinary,
    RequestAccessPolicy,
}
struct PreparedIp {
    root: usize,
    leaf: Option<LeafPosition>,
    slots: Vec<PreparedSlot>,
}
impl PreparedIp {
    fn new(owner: &OwnerSnapshot, root: usize, leaf: Option<LeafPosition>, ip: &IpReferenceSet) -> Self {
        Self {
            root,
            leaf,
            slots: ip
                .ip_refs
                .iter()
                .enumerate()
                .map(|(reference, wire)| {
                    PreparedSlot::new(
                        owner,
                        ip_wire(wire),
                        FieldContract::FixedConfigData,
                        FieldSlot::ConditionIp { root, leaf, reference },
                    )
                })
                .collect(),
        }
    }
    fn matches(&self, ip: &IpReferenceSet) -> bool {
        self.slots.len() == ip.ip_refs.len()
            && self.slots.iter().zip(&ip.ip_refs).all(|(slot, wire)| {
                matches!(&slot.wire, FieldWire::ConfigData { name, namespace }
                        if name == &wire.name && namespace == &wire.namespace)
            })
    }
}
fn ip_wire(wire: &EdgionConfigDataRef) -> FieldWire {
    FieldWire::ConfigData {
        name: wire.name.clone(),
        namespace: wire.namespace.clone(),
    }
}
pub struct BoundIpAccess<'a> {
    config: &'a IpReferenceSet,
    prepared: &'a PreparedIp,
    snapshot: &'a ConditionSnapshot,
    consumer: IpReferenceConsumer,
    role: IpExecutionRole,
    supported: bool,
}
pub struct BoundIpAccessMut<'a> {
    config: &'a mut IpReferenceSet,
    prepared: &'a PreparedIp,
    snapshot: &'a ConditionSnapshot,
    consumer: IpReferenceConsumer,
    role: IpExecutionRole,
    supported: bool,
}
macro_rules! ip_access {
    () => {
        pub fn config(&self) -> &IpReferenceSet {
            self.config
        }
        pub fn validate_owner(&self) -> Result<(), ReferenceError> {
            if !self.prepared.matches(self.config) {
                return Err(ReferenceError::StaleBinding);
            }
            self.snapshot.owner.source().map(|_| ())
        }

        fn resolution(&self, scopes: &[IpReadScope]) -> Result<IpReadResolution, ReferenceError> {
            self.validate_owner()?;
            if !self.prepared.matches(self.config) || scopes.len() != self.prepared.slots.len() {
                return Err(ReferenceError::StaleBinding);
            }
            let leaf = self.prepared.leaf.map(|leaf| match leaf {
                LeafPosition::AllOf(index) => (index, None),
                LeafPosition::AnyOf(index) => (index, Some(usize::MAX)),
                LeafPosition::AnyClauseAll { clause, leaf } => (clause, Some(leaf)),
            });
            super::owned::ip_read_resolution(
                &self.snapshot.owner,
                IpReadDomain::Condition,
                self.prepared.root,
                leaf,
                self.references()?,
                scopes,
            )
        }
        /// Read proof must match the actual source, owning slot and canonical target.
        pub fn read_scopes(&self) -> Result<Option<Vec<IpReadScope>>, ReferenceError> {
            let Some(resolution) = self.config.resolved_refs.as_deref() else {
                return Ok(None);
            };
            let scopes: Vec<_> = resolution.slots.iter().map(|slot| slot.scope).collect();
            if self.resolution(&scopes)? != *resolution {
                return Err(ReferenceError::StaleBinding);
            }
            Ok(Some(scopes))
        }
        pub fn is_supported(&self) -> bool {
            self.supported
        }
        pub fn execution_role(&self) -> IpExecutionRole {
            self.role
        }
        pub fn consumer(&self) -> IpReferenceConsumer {
            self.consumer
        }
        pub fn references(&self) -> Result<impl Iterator<Item = ReferenceToken<'_>>, ReferenceError> {
            if !self.prepared.matches(self.config) {
                return Err(ReferenceError::StaleBinding);
            }
            Ok(self
                .prepared
                .slots
                .iter()
                .map(|slot| slot.token(TopologyLoan::Conditions(self.snapshot))))
        }
    };
}
impl BoundIpAccess<'_> {
    ip_access!();
}
impl BoundIpAccessMut<'_> {
    ip_access!();
    pub fn set_read_scopes(&mut self, scopes: &[IpReadScope]) -> Result<(), ReferenceError> {
        let resolution = self.resolution(scopes)?;
        self.config.resolved_refs = Some(Box::new(resolution));
        Ok(())
    }
    pub fn config_mut(&mut self) -> &mut IpReferenceSet {
        self.config
    }
}
impl PreparedRoot {
    fn ip_role(&self) -> IpExecutionRole {
        match self.position.location {
            Location::RealIp => IpExecutionRole::Trust,
            Location::RapIp { .. } | Location::RapUrl { .. } => IpExecutionRole::Partial,
            Location::Dye { .. } => IpExecutionRole::Observational,
            Location::Restriction { deny: false } => IpExecutionRole::Candidate,
            _ => IpExecutionRole::Required,
        }
    }
    fn ip_consumer(&self) -> Option<IpReferenceConsumer> {
        if !self.enabled {
            return None;
        }
        match self.position.location {
            Location::RealIp if self.position.stage == PluginStage::Request && self.supported => {
                Some(IpReferenceConsumer::RealIp)
            }
            Location::RealIp => None,
            Location::RapIp { .. } | Location::RapUrl { .. } if self.position.stage == PluginStage::Request => {
                Some(IpReferenceConsumer::RequestAccessPolicy)
            }
            Location::RapIp { .. } | Location::RapUrl { .. } | Location::Boundary { .. } => None,
            Location::Restriction { .. } if self.position.stage != PluginStage::Request => None,
            _ => Some(IpReferenceConsumer::Ordinary),
        }
    }
}
impl PreparedConditionReferences {
    pub fn visit_ip(
        &self,
        owner: &EdgionPlugins,
        mut visit: impl for<'leaf> FnMut(BoundIpAccess<'leaf>),
    ) -> Result<(), ReferenceError> {
        self.check(owner)?;
        for (root, prepared) in roots(owner).into_iter().zip(&self.snapshot.roots) {
            let Some(consumer) = prepared.ip_consumer() else {
                continue;
            };
            if let (Some(config), Some(ip)) = (direct_ip(owner, &root.position), &prepared.ip) {
                visit(BoundIpAccess {
                    config,
                    prepared: ip,
                    snapshot: &self.snapshot,
                    consumer,
                    role: prepared.ip_role(),
                    supported: prepared.supported,
                });
            }
            let role = prepared.ip_role();
            let supported = prepared.supported;
            if let (Some(set), Some(prepared)) = (root.conditions, &prepared.set) {
                for leaf in &prepared.leaves {
                    if let Some(ip) = &leaf.ip {
                        let config = condition_ip(leaf_at(set, leaf.position).ok_or(ReferenceError::StaleBinding)?)
                            .ok_or(ReferenceError::StaleBinding)?;
                        visit(BoundIpAccess {
                            config,
                            prepared: ip,
                            snapshot: &self.snapshot,
                            consumer,
                            role,
                            supported,
                        });
                    }
                }
            }
        }
        Ok(())
    }
    /// Clear resolver-only metadata on every effective owning node, including inactive roots.
    /// Clearing never mints a reference token or performs a lookup.
    pub fn clear_ip_read_scopes(&self, owner: &mut EdgionPlugins) -> Result<(), ReferenceError> {
        self.check(owner)?;
        let clear = |config: &mut IpReferenceSet| {
            config.resolved_refs = None;
            for reference in &mut config.ip_refs {
                reference.ref_denied = None;
            }
        };
        for root in &self.snapshot.roots {
            if root.ip.is_some() {
                clear(direct_ip_mut(owner, &root.position).ok_or(ReferenceError::StaleBinding)?);
            }
            if let Some(set) = &root.set {
                let actual = root_mut(owner, &root.position).ok_or(ReferenceError::StaleBinding)?;
                for leaf in &set.leaves {
                    if leaf.ip.is_some() {
                        clear(
                            condition_ip_mut(leaf_at_mut(actual, leaf.position).ok_or(ReferenceError::StaleBinding)?)
                                .ok_or(ReferenceError::StaleBinding)?,
                        );
                    }
                }
            }
        }
        Ok(())
    }

    /// Warning application rechecks the same owning topology before borrowing the RAP entry.
    pub fn visit_ip_mut(
        &self,
        owner: &mut EdgionPlugins,
        mut visit: impl for<'leaf> FnMut(BoundIpAccessMut<'leaf>, &mut dyn FnMut(String)),
        mut append_warning: impl FnMut(&mut Vec<String>, String),
    ) -> Result<(), ReferenceError> {
        self.check(owner)?;
        let mut pending = std::collections::BTreeMap::new();
        for root in &self.snapshot.roots {
            let Some(consumer) = root.ip_consumer() else {
                continue;
            };
            if consumer == IpReferenceConsumer::RequestAccessPolicy && !pending.contains_key(&root.position.entry) {
                pending.insert(
                    root.position.entry,
                    rap_warnings_mut(owner, root.position.entry)?.clone(),
                );
            }
            let mut emit = |warning| {
                if consumer == IpReferenceConsumer::RequestAccessPolicy {
                    if let Some(warnings) = pending.get_mut(&root.position.entry) {
                        append_warning(warnings, warning);
                    }
                }
            };
            if let Some(prepared) = &root.ip {
                let config = direct_ip_mut(owner, &root.position).ok_or(ReferenceError::StaleBinding)?;
                visit(
                    BoundIpAccessMut {
                        config,
                        prepared,
                        snapshot: &self.snapshot,
                        consumer,
                        role: root.ip_role(),
                        supported: root.supported,
                    },
                    &mut emit,
                );
            }
            if let Some(set) = &root.set {
                let config = root_mut(owner, &root.position).ok_or(ReferenceError::StaleBinding)?;
                for leaf in &set.leaves {
                    if let Some(prepared) = &leaf.ip {
                        let config =
                            condition_ip_mut(leaf_at_mut(config, leaf.position).ok_or(ReferenceError::StaleBinding)?)
                                .ok_or(ReferenceError::StaleBinding)?;
                        visit(
                            BoundIpAccessMut {
                                config,
                                prepared,
                                snapshot: &self.snapshot,
                                consumer,
                                role: root.ip_role(),
                                supported: root.supported,
                            },
                            &mut emit,
                        );
                    }
                }
            }
        }
        self.check(owner)?;
        for (entry, warnings) in pending {
            *rap_warnings_mut(owner, entry)? = warnings;
        }
        Ok(())
    }
}
fn rap_warnings_mut(owner: &mut EdgionPlugins, entry: usize) -> Result<&mut Vec<String>, ReferenceError> {
    let plugin = &mut owner
        .spec
        .request_plugins
        .as_mut()
        .ok_or(ReferenceError::StaleBinding)?
        .get_mut(entry)
        .ok_or(ReferenceError::StaleBinding)?
        .plugin;
    let EdgionPlugin::RequestAccessPolicy(config) = plugin else {
        return Err(ReferenceError::StaleBinding);
    };
    Ok(&mut config.resolution_warnings)
}

fn condition_ip(condition: &Condition) -> Option<&IpReferenceSet> {
    match condition {
        Condition::DirectPeerIpMatch(ip) | Condition::RemoteIpMatch(ip) => Some(&ip.refs),
        _ => None,
    }
}
fn condition_ip_mut(condition: &mut Condition) -> Option<&mut IpReferenceSet> {
    match condition {
        Condition::DirectPeerIpMatch(ip) | Condition::RemoteIpMatch(ip) => Some(&mut ip.refs),
        _ => None,
    }
}
fn direct_ip<'a>(owner: &'a EdgionPlugins, position: &RootPosition) -> Option<&'a IpReferenceSet> {
    let plugin = match position.stage {
        PluginStage::Request => &owner.spec.request_plugins.as_ref()?.get(position.entry)?.plugin,
        PluginStage::Response => {
            &owner
                .spec
                .upstream_response_plugins
                .as_ref()?
                .get(position.entry)?
                .plugin
        }
        PluginStage::ResponseFilter => {
            &owner
                .spec
                .upstream_response_filter_plugins
                .as_ref()?
                .get(position.entry)?
                .plugin
        }
        PluginStage::ResponseBodyFilter => {
            &owner
                .spec
                .upstream_response_body_filter_plugins
                .as_ref()?
                .get(position.entry)?
                .plugin
        }
    };
    if matches!(position.location, Location::RealIp) {
        let EdgionPlugin::RealIp(config) = plugin else {
            return None;
        };
        return Some(&config.trusted);
    }
    let Location::RapIp {
        profile,
        group,
        rule,
        direct,
    } = &position.location
    else {
        return None;
    };
    let EdgionPlugin::RequestAccessPolicy(config) = plugin else {
        return None;
    };
    let rule = &config
        .resolved_profiles
        .as_ref()?
        .get(profile)?
        .required_rule_groups
        .get(*group)?
        .any_of_rules
        .get(*rule)?
        .rule;
    match rule {
        RequestAccessRuleType::DirectPeerIp(ip) if *direct => Some(&ip.refs),
        RequestAccessRuleType::RemoteIp(ip) if !*direct => Some(&ip.refs),
        _ => None,
    }
}
fn direct_ip_mut<'a>(owner: &'a mut EdgionPlugins, position: &RootPosition) -> Option<&'a mut IpReferenceSet> {
    let plugin = match position.stage {
        PluginStage::Request => &mut owner.spec.request_plugins.as_mut()?.get_mut(position.entry)?.plugin,
        PluginStage::Response => {
            &mut owner
                .spec
                .upstream_response_plugins
                .as_mut()?
                .get_mut(position.entry)?
                .plugin
        }
        PluginStage::ResponseFilter => {
            &mut owner
                .spec
                .upstream_response_filter_plugins
                .as_mut()?
                .get_mut(position.entry)?
                .plugin
        }
        PluginStage::ResponseBodyFilter => {
            &mut owner
                .spec
                .upstream_response_body_filter_plugins
                .as_mut()?
                .get_mut(position.entry)?
                .plugin
        }
    };
    if matches!(position.location, Location::RealIp) {
        let EdgionPlugin::RealIp(config) = plugin else {
            return None;
        };
        return Some(&mut config.trusted);
    }
    let Location::RapIp {
        profile,
        group,
        rule,
        direct,
    } = &position.location
    else {
        return None;
    };
    let EdgionPlugin::RequestAccessPolicy(config) = plugin else {
        return None;
    };
    let rule = &mut config
        .resolved_profiles
        .as_mut()?
        .get_mut(profile)?
        .required_rule_groups
        .get_mut(*group)?
        .any_of_rules
        .get_mut(*rule)?
        .rule;
    match rule {
        RequestAccessRuleType::DirectPeerIp(ip) if *direct => Some(&mut ip.refs),
        RequestAccessRuleType::RemoteIp(ip) if !*direct => Some(&mut ip.refs),
        _ => None,
    }
}

pub struct BoundCondition<'a> {
    config: &'a Condition,
    prepared: &'a PreparedLeaf,
    snapshot: &'a ConditionSnapshot,
    active: bool,
}
pub struct BoundConditionMut<'a> {
    config: &'a mut Condition,
    prepared: &'a PreparedLeaf,
    snapshot: &'a ConditionSnapshot,
    active: bool,
}
macro_rules! access {
    () => {
        pub fn config(&self) -> &Condition {
            self.config
        }
        pub fn is_active(&self) -> bool {
            self.active
        }
        pub fn references(&self) -> Result<ConditionReferences<'_>, ReferenceError> {
            if !self.prepared.matches(self.config) {
                return Err(ReferenceError::StaleBinding);
            }
            Ok(ConditionReferences {
                prepared: self.prepared,
                snapshot: self.snapshot,
                active: self.active,
            })
        }
    };
}
impl BoundCondition<'_> {
    access!();
}
impl BoundConditionMut<'_> {
    access!();
    pub fn config_mut(&mut self) -> &mut Condition {
        self.config
    }
}
pub struct ConditionReferences<'a> {
    prepared: &'a PreparedLeaf,
    snapshot: &'a ConditionSnapshot,
    active: bool,
}
pub struct HmacGroupReferences<'a> {
    group: &'a PreparedGroup,
    snapshot: &'a ConditionSnapshot,
}
impl ConditionReferences<'_> {
    pub fn secret_match_secret(&self) -> Option<ReferenceToken<'_>> {
        self.active
            .then_some(self.prepared.secret.as_ref())
            .flatten()
            .map(|slot| slot.token(TopologyLoan::Conditions(self.snapshot)))
    }
    pub fn hmac_secret_groups(&self) -> impl Iterator<Item = HmacGroupReferences<'_>> {
        self.prepared
            .groups
            .iter()
            .flatten()
            .filter(move |_| self.active)
            .map(|group| HmacGroupReferences {
                group,
                snapshot: self.snapshot,
            })
    }
}
impl HmacGroupReferences<'_> {
    pub fn name(&self) -> &str {
        &self.group.name
    }
    pub fn references(&self) -> impl Iterator<Item = ReferenceToken<'_>> {
        self.group
            .slots
            .iter()
            .map(|slot| slot.token(TopologyLoan::Conditions(self.snapshot)))
    }
}

impl PreparedSet {
    fn new(owner: &OwnerSnapshot, root: usize, set: &ConditionSet) -> Self {
        Self {
            shape: shape(set),
            leaves: leaves(set)
                .into_iter()
                .map(|(position, condition)| {
                    let slot = |wire: &crate::resources::common::SecretObjectReference, group, reference| {
                        PreparedSlot::new(
                            owner,
                            FieldWire::Secret(wire.clone()),
                            FieldContract::CustomSecret,
                            FieldSlot::Condition {
                                root,
                                leaf: position,
                                group,
                                reference,
                            },
                        )
                    };
                    let (key, secret, groups) = match condition {
                        Condition::SecretMatch(config) => (
                            Some(config.secret_key.clone()),
                            Some(slot(&config.secret_ref, None, 0)),
                            None,
                        ),
                        Condition::HmacAuth(config) => (
                            None,
                            None,
                            config.secret_groups.as_ref().map(|groups| {
                                groups
                                    .iter()
                                    .enumerate()
                                    .map(|(group_index, group)| PreparedGroup {
                                        name: group.name.clone(),
                                        slots: group
                                            .secret_refs
                                            .iter()
                                            .enumerate()
                                            .map(|(ordinal, wire)| slot(wire, Some(group_index), ordinal))
                                            .collect(),
                                    })
                                    .collect()
                            }),
                        ),
                        _ => (None, None, None),
                    };
                    PreparedLeaf {
                        position,
                        variant: discriminant(condition),
                        key,
                        secret,
                        groups,
                        ip: condition_ip(condition).map(|ip| PreparedIp::new(owner, root, Some(position), ip)),
                    }
                })
                .collect(),
        }
    }
    fn matches(&self, set: &ConditionSet) -> bool {
        if self.shape != shape(set) {
            return false;
        }
        let actual = leaves(set);
        actual.len() == self.leaves.len()
            && actual
                .into_iter()
                .zip(&self.leaves)
                .all(|((position, condition), prepared)| position == prepared.position && prepared.matches(condition))
    }
}
impl PreparedLeaf {
    fn matches(&self, condition: &Condition) -> bool {
        if self.variant != discriminant(condition) {
            return false;
        }
        match condition {
            Condition::DirectPeerIpMatch(ip) | Condition::RemoteIpMatch(ip) => {
                self.ip.as_ref().is_some_and(|prepared| prepared.matches(&ip.refs))
            }
            Condition::SecretMatch(config) => {
                self.key.as_deref() == Some(&config.secret_key)
                    && self
                        .secret
                        .as_ref()
                        .is_some_and(|slot| matches!(&slot.wire, FieldWire::Secret(wire) if wire == &config.secret_ref))
            }
            Condition::HmacAuth(config) => match (&self.groups, &config.secret_groups) {
                (None, None) => true,
                (Some(prepared), Some(groups)) => {
                    prepared.len() == groups.len()
                        && prepared.iter().zip(groups).all(|(prepared, group)| {
                            prepared.name == group.name
                            && prepared.slots.len() == group.secret_refs.len()
                            && prepared.slots.iter().zip(&group.secret_refs).all(
                                |(slot, wire)| matches!(&slot.wire, FieldWire::Secret(prepared) if prepared == wire),
                            )
                        })
                }
                _ => false,
            },
            _ => true,
        }
    }
}
fn shape(set: &ConditionSet) -> Shape {
    match set {
        ConditionSet::AllOf { all_of } => Shape::AllOf(all_of.len()),
        ConditionSet::AnyOf { any_of } => Shape::AnyOf(
            any_of
                .iter()
                .map(|clause| match clause {
                    AnyOfClause::Condition(_) => ClauseShape::Direct,
                    AnyOfClause::AllOf(group) => ClauseShape::AllOf {
                        name: group.name.clone(),
                        length: group.all_of.len(),
                    },
                })
                .collect(),
        ),
    }
}
fn leaves(set: &ConditionSet) -> Vec<(LeafPosition, &Condition)> {
    match set {
        ConditionSet::AllOf { all_of } => all_of
            .iter()
            .enumerate()
            .map(|(index, condition)| (LeafPosition::AllOf(index), condition))
            .collect(),
        ConditionSet::AnyOf { any_of } => any_of
            .iter()
            .enumerate()
            .flat_map(|(clause, value)| match value {
                AnyOfClause::Condition(condition) => vec![(LeafPosition::AnyOf(clause), condition)],
                AnyOfClause::AllOf(group) => group
                    .all_of
                    .iter()
                    .enumerate()
                    .map(|(leaf, condition)| (LeafPosition::AnyClauseAll { clause, leaf }, condition))
                    .collect(),
            })
            .collect(),
    }
}
fn leaf_at(set: &ConditionSet, position: LeafPosition) -> Option<&Condition> {
    match (set, position) {
        (ConditionSet::AllOf { all_of }, LeafPosition::AllOf(index)) => all_of.get(index),
        (ConditionSet::AnyOf { any_of }, LeafPosition::AnyOf(index)) => match any_of.get(index)? {
            AnyOfClause::Condition(condition) => Some(condition),
            _ => None,
        },
        (ConditionSet::AnyOf { any_of }, LeafPosition::AnyClauseAll { clause, leaf }) => match any_of.get(clause)? {
            AnyOfClause::AllOf(group) => group.all_of.get(leaf),
            _ => None,
        },
        _ => None,
    }
}
fn leaf_at_mut(set: &mut ConditionSet, position: LeafPosition) -> Option<&mut Condition> {
    match (set, position) {
        (ConditionSet::AllOf { all_of }, LeafPosition::AllOf(index)) => all_of.get_mut(index),
        (ConditionSet::AnyOf { any_of }, LeafPosition::AnyOf(index)) => match any_of.get_mut(index)? {
            AnyOfClause::Condition(condition) => Some(condition),
            _ => None,
        },
        (ConditionSet::AnyOf { any_of }, LeafPosition::AnyClauseAll { clause, leaf }) => {
            match any_of.get_mut(clause)? {
                AnyOfClause::AllOf(group) => group.all_of.get_mut(leaf),
                _ => None,
            }
        }
        _ => None,
    }
}
fn stage_shape(owner: &EdgionPlugins) -> [Option<usize>; 4] {
    [
        owner.spec.request_plugins.as_ref().map(Vec::len),
        owner.spec.upstream_response_filter_plugins.as_ref().map(Vec::len),
        owner.spec.upstream_response_body_filter_plugins.as_ref().map(Vec::len),
        owner.spec.upstream_response_plugins.as_ref().map(Vec::len),
    ]
}

// Catalogue every actual root, including empty containers and inactive entries.
// Profile keys are sorted, so insertion order cannot change provenance.
fn roots(owner: &EdgionPlugins) -> Vec<Root<'_>> {
    let mut roots = Vec::new();
    macro_rules! stage {
        ($entries:expr, $stage:expr, $body:expr) => {
            for (index, entry) in $entries.iter().flatten().enumerate() {
                let stage = $stage;
                let enabled = entry.enable;
                let supported = supports_stage(stage, &entry.plugin);
                let variant = entry.plugin.type_name();
                let mut push = |location, action, conditions| {
                    roots.push(Root {
                        position: RootPosition {
                            stage,
                            entry: index,
                            location,
                            action,
                        },
                        enabled,
                        supported,
                        variant,
                        conditions,
                    })
                };
                plugin_sets(
                    entry.conditions.as_ref(),
                    Location::Entry {
                        present: entry.conditions.is_some(),
                    },
                    &mut push,
                );
                let body: Option<&crate::resources::edgion_plugins::body_requirement::BodyRequirement> = $body(entry);
                if matches!(stage, PluginStage::Request | PluginStage::Response) {
                    plugin_sets(
                        body.and_then(|body| body.conditions.as_ref()),
                        Location::Body {
                            present: body.is_some(),
                            conditions: body.is_some_and(|body| body.conditions.is_some()),
                        },
                        &mut push,
                    );
                }
                dye_sets(entry.dye.as_ref(), &mut push);
                {
                    match &entry.plugin {
                        EdgionPlugin::RealIp(_) => push(Location::RealIp, Action::Direct, None),
                        EdgionPlugin::RequestRestriction(config) => push(
                            Location::Restriction {
                                deny: config.mode
                                    == Some(crate::resources::edgion_plugins::plugin_configs::RestrictionMode::Deny),
                            },
                            Action::Direct,
                            Some(&config.conditions),
                        ),
                        EdgionPlugin::RequestAccessPolicy(config) => {
                            push(
                                Location::Boundary {
                                    tag: "rap-profiles",
                                    key: None,
                                    first: usize::from(config.resolved_profiles.is_some()),
                                    second: 0,
                                },
                                Action::Direct,
                                None,
                            );
                            if let Some(profiles) = config.resolved_profiles.as_ref() {
                                let mut keys: Vec<_> = profiles.keys().collect();
                                keys.sort();
                                for key in keys {
                                    let profile = &profiles[key];
                                    push(
                                        Location::Boundary {
                                            tag: "rap-profile",
                                            key: Some(key.clone()),
                                            first: profile.required_rule_groups.len(),
                                            second: 0,
                                        },
                                        Action::Direct,
                                        None,
                                    );
                                    for (group, gate) in profile.required_rule_groups.iter().enumerate() {
                                        push(
                                            Location::Boundary {
                                                tag: "rap-group",
                                                key: Some(gate.name.clone()),
                                                first: group,
                                                second: gate.any_of_rules.len(),
                                            },
                                            Action::Direct,
                                            None,
                                        );
                                        for (rule, item) in gate.any_of_rules.iter().enumerate() {
                                            push(
                                                Location::Boundary {
                                                    tag: "rap-rule",
                                                    key: Some(item.name.clone()),
                                                    first: group,
                                                    second: rule,
                                                },
                                                Action::Direct,
                                                None,
                                            );
                                            match &item.rule {
                                                RequestAccessRuleType::DirectPeerIp(ip)
                                                | RequestAccessRuleType::RemoteIp(ip) => {
                                                    push(
                                                        Location::RapIp {
                                                            profile: key.clone(),
                                                            group,
                                                            rule,
                                                            direct: matches!(
                                                                item.rule,
                                                                RequestAccessRuleType::DirectPeerIp(_)
                                                            ),
                                                        },
                                                        Action::Direct,
                                                        ip.conditions.as_ref(),
                                                    );
                                                }
                                                RequestAccessRuleType::Url(url) => {
                                                    push(
                                                        Location::Boundary {
                                                            tag: "rap-url-candidates",
                                                            key: None,
                                                            first: usize::from(url.resolved_candidates.is_some()),
                                                            second: url
                                                                .resolved_candidates
                                                                .as_ref()
                                                                .map_or(0, Vec::len),
                                                        },
                                                        Action::Direct,
                                                        None,
                                                    );
                                                    for (candidate, value) in
                                                        url.resolved_candidates.iter().flatten().enumerate()
                                                    {
                                                        push(
                                                            Location::Boundary {
                                                                tag: "rap-url-candidate",
                                                                key: Some(value.name.clone()),
                                                                first: rule,
                                                                second: candidate,
                                                            },
                                                            Action::Direct,
                                                            None,
                                                        );
                                                        push(
                                                            Location::RapUrl {
                                                                profile: key.clone(),
                                                                group,
                                                                rule,
                                                                candidate,
                                                            },
                                                            Action::Direct,
                                                            value.conditions.as_ref(),
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        };
    }
    stage!(owner.spec.request_plugins, PluginStage::Request, request_body);
    stage!(
        owner.spec.upstream_response_filter_plugins,
        PluginStage::ResponseFilter,
        no_body
    );
    stage!(
        owner.spec.upstream_response_body_filter_plugins,
        PluginStage::ResponseBodyFilter,
        no_body
    );
    stage!(
        owner.spec.upstream_response_plugins,
        PluginStage::Response,
        response_body
    );
    roots
}
fn plugin_sets<'a>(
    sets: Option<&'a PluginConditions>,
    location: Location,
    push: &mut impl FnMut(Location, Action, Option<&'a ConditionSet>),
) {
    push(location.clone(), Action::Skip, sets.and_then(|sets| sets.skip.as_ref()));
    push(location, Action::Run, sets.and_then(|sets| sets.run.as_ref()));
}
fn dye_sets<'a>(dye: Option<&'a DyeSuite>, push: &mut impl FnMut(Location, Action, Option<&'a ConditionSet>)) {
    push(
        Location::Boundary {
            tag: "dye",
            key: None,
            first: usize::from(dye.is_some()),
            second: 0,
        },
        Action::Direct,
        None,
    );
    if let Some(dye) = dye {
        for (response, rules) in [(false, &dye.request), (true, &dye.response)] {
            push(
                Location::Boundary {
                    tag: if response { "response-dye" } else { "request-dye" },
                    key: None,
                    first: rules.len(),
                    second: 0,
                },
                Action::Direct,
                None,
            );
            for (rule, value) in rules.iter().enumerate() {
                plugin_sets(
                    value.conditions.as_ref(),
                    Location::Dye {
                        response,
                        rule,
                        conditions: value.conditions.is_some(),
                    },
                    push,
                );
            }
        }
    }
}

fn selected_set(sets: &mut PluginConditions, action: Action) -> Option<&mut ConditionSet> {
    match action {
        Action::Skip => sets.skip.as_mut(),
        Action::Run => sets.run.as_mut(),
        Action::Direct => None,
    }
}
fn root_mut<'a>(owner: &'a mut EdgionPlugins, position: &RootPosition) -> Option<&'a mut ConditionSet> {
    macro_rules! entry_root {
        ($entries:expr, $body:expr) => {{
            let entry = $entries.as_mut()?.get_mut(position.entry)?;
            match &position.location {
                Location::Entry { .. } => selected_set(entry.conditions.as_mut()?, position.action),
                Location::Dye { response, rule, .. } => {
                    let dye = entry.dye.as_mut()?;
                    let rules = if *response { &mut dye.response } else { &mut dye.request };
                    selected_set(rules.get_mut(*rule)?.conditions.as_mut()?, position.action)
                }
                _ => $body(entry, position),
            }
        }};
    }
    match position.stage {
        PluginStage::Request => entry_root!(owner.spec.request_plugins, request_special_mut),
        PluginStage::ResponseFilter => entry_root!(owner.spec.upstream_response_filter_plugins, filter_special_mut),
        PluginStage::ResponseBodyFilter => entry_root!(
            owner.spec.upstream_response_body_filter_plugins,
            body_filter_special_mut
        ),
        PluginStage::Response => entry_root!(owner.spec.upstream_response_plugins, response_special_mut),
    }
}

fn request_body(
    entry: &crate::resources::edgion_plugins::RequestFilterEntry,
) -> Option<&crate::resources::edgion_plugins::body_requirement::BodyRequirement> {
    entry.body.as_ref()
}
fn response_body(
    entry: &crate::resources::edgion_plugins::UpstreamResponseEntry,
) -> Option<&crate::resources::edgion_plugins::body_requirement::BodyRequirement> {
    entry.body.as_ref()
}
fn no_body<T>(_entry: &T) -> Option<&crate::resources::edgion_plugins::body_requirement::BodyRequirement> {
    None
}
fn filter_special_mut<'a>(
    entry: &'a mut crate::resources::edgion_plugins::UpstreamResponseFilterEntry,
    position: &RootPosition,
) -> Option<&'a mut ConditionSet> {
    plugin_root_mut(&mut entry.plugin, position)
}
fn body_filter_special_mut<'a>(
    entry: &'a mut crate::resources::edgion_plugins::UpstreamResponseBodyFilterEntry,
    position: &RootPosition,
) -> Option<&'a mut ConditionSet> {
    plugin_root_mut(&mut entry.plugin, position)
}
fn response_special_mut<'a>(
    entry: &'a mut crate::resources::edgion_plugins::UpstreamResponseEntry,
    position: &RootPosition,
) -> Option<&'a mut ConditionSet> {
    if matches!(position.location, Location::Body { .. }) {
        selected_set(entry.body.as_mut()?.conditions.as_mut()?, position.action)
    } else {
        plugin_root_mut(&mut entry.plugin, position)
    }
}
fn request_special_mut<'a>(
    entry: &'a mut crate::resources::edgion_plugins::RequestFilterEntry,
    position: &RootPosition,
) -> Option<&'a mut ConditionSet> {
    if matches!(position.location, Location::Body { .. }) {
        selected_set(entry.body.as_mut()?.conditions.as_mut()?, position.action)
    } else {
        plugin_root_mut(&mut entry.plugin, position)
    }
}
fn plugin_root_mut<'a>(plugin: &'a mut EdgionPlugin, position: &RootPosition) -> Option<&'a mut ConditionSet> {
    match &position.location {
        Location::Restriction { .. } => match plugin {
            EdgionPlugin::RequestRestriction(config) => Some(&mut config.conditions),
            _ => None,
        },
        Location::RapIp {
            profile,
            group,
            rule,
            direct,
        } => {
            let EdgionPlugin::RequestAccessPolicy(config) = plugin else {
                return None;
            };
            let rule = &mut config
                .resolved_profiles
                .as_mut()?
                .get_mut(profile)?
                .required_rule_groups
                .get_mut(*group)?
                .any_of_rules
                .get_mut(*rule)?
                .rule;
            match rule {
                RequestAccessRuleType::DirectPeerIp(ip) if *direct => ip.conditions.as_mut(),
                RequestAccessRuleType::RemoteIp(ip) if !*direct => ip.conditions.as_mut(),
                _ => None,
            }
        }
        Location::RapUrl {
            profile,
            group,
            rule,
            candidate,
        } => {
            let EdgionPlugin::RequestAccessPolicy(config) = plugin else {
                return None;
            };
            let RequestAccessRuleType::Url(url) = &mut config
                .resolved_profiles
                .as_mut()?
                .get_mut(profile)?
                .required_rule_groups
                .get_mut(*group)?
                .any_of_rules
                .get_mut(*rule)?
                .rule
            else {
                return None;
            };
            url.resolved_candidates
                .as_mut()?
                .get_mut(*candidate)?
                .conditions
                .as_mut()
        }
        _ => None,
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/condition_fields_test.rs"]
mod tests;
