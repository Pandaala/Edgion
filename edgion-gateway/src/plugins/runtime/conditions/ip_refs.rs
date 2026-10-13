//! Actual-owner cold binding and scalar reads from one dispatch-local view.
use super::MatchState;
use crate::plugins::common::ip_acl::CompiledIpAcl;
use crate::plugins::runtime::edgion_config_data::EdgionConfigDataSnapshot;
use crate::plugins::runtime::PluginSession;
use edgion_resources::refs::condition_fields::{BoundIpAccess, IpExecutionRole, PreparedConditionReferences};
use edgion_resources::refs::ip_list_fields::{BoundGeoIpAccess, PreparedIpListReferences};
use edgion_resources::refs::{ReferenceError, ReferenceToken};
use edgion_resources::resources::edgion_plugins::ip_acl_refs::IpAclReferenceSet;
use edgion_resources::resources::edgion_plugins::ip_reference_set::{IpReadScope, IpReferenceSet};
use edgion_resources::{EdgionGatewayConfig, EdgionPlugins};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::net::IpAddr;
use std::sync::Arc;

/// Identity and read policy compiled only from an actual owning reference loan.
#[derive(Debug)]
pub struct IpListReadKey {
    pub(crate) key: String,
    pub(crate) scope: IpReadScope,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpListAvailability {
    Available,
    Unavailable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSelection {
    Selected(usize),
    Unavailable,
    WrongType,
    UnknownProfile,
}

pub(crate) struct BoundIpPredicate {
    keys: Vec<IpListReadKey>,
    role: IpExecutionRole,
    invalid: bool,
}
impl BoundIpPredicate {
    pub(crate) fn required(&self) -> bool {
        self.role == IpExecutionRole::Required
    }
    fn available_by(&self, mut read: impl FnMut(&IpListReadKey) -> IpListAvailability) -> bool {
        !self.invalid && self.keys.iter().all(|key| read(key) == IpListAvailability::Available)
    }
    pub(crate) fn available(&self, session: &dyn PluginSession) -> bool {
        self.available_by(|key| session.ip_list_availability(key))
    }
    pub(crate) fn available_in(&self, view: &EdgionConfigDataSnapshot) -> bool {
        self.available_by(|key| view.ip_list_availability(key))
    }
    fn contains_by(&self, ip: IpAddr, mut read: impl FnMut(&IpListReadKey, IpAddr) -> Option<bool>) -> bool {
        self.keys.iter().any(|key| read(key, ip) == Some(true))
    }
    pub(crate) fn contains(&self, session: &dyn PluginSession, ip: IpAddr) -> bool {
        self.contains_by(ip, |key, ip| session.ip_list_contains(key, ip))
    }
    pub(crate) fn contains_in(&self, view: &EdgionConfigDataSnapshot, ip: IpAddr) -> bool {
        self.contains_by(ip, |key, ip| view.ip_list_contains(key, ip))
    }
    pub(crate) fn evaluate(&self, session: &dyn PluginSession, ip: Option<IpAddr>, negate: bool) -> MatchState {
        if self.invalid {
            return MatchState::Unresolved;
        }
        let partial = self.role == IpExecutionRole::Partial;
        if !partial && !self.available(session) {
            return MatchState::Unresolved;
        }
        let matched = ip.is_some_and(|ip| self.contains(session, ip));
        if matched != negate {
            MatchState::True
        } else {
            MatchState::False
        }
    }
}

/// Addresses index only original borrowed nodes during construction; none escape.
pub(crate) struct IpConditionCompiler<'owner> {
    bindings: HashMap<usize, Arc<BoundIpPredicate>>,
    acls: HashMap<usize, Arc<CompiledIpAcl>>,
    required: Vec<Arc<BoundIpPredicate>>,
    _owner: PhantomData<&'owner ()>,
}
impl<'owner> IpConditionCompiler<'owner> {
    pub(crate) fn new(owner: &'owner EdgionPlugins) -> Result<Self, ReferenceError> {
        let prepared = PreparedConditionReferences::from_plugins(owner);
        let mut compiler = Self::from_visitor(|visitor| prepared.visit_ip(owner, visitor))?;
        let geo = PreparedIpListReferences::from_plugins(owner);
        let mut error = None;
        let binding = geo.bind_plugins(owner)?;
        for (index, entry) in owner.spec.request_plugins.iter().flatten().enumerate() {
            if !entry.enable {
                continue;
            }
            if let edgion_resources::resources::edgion_plugins::EdgionPlugin::IpRestriction(config) = &entry.plugin {
                if config.refs.validation_error().is_some() {
                    continue;
                }
                let entry = binding.http_entry(index).ok_or(ReferenceError::StaleBinding)?;
                let plan = CompiledIpAcl::from_bound(entry.references(), &config.default_action)?;
                compiler
                    .acls
                    .insert(&config.refs as *const IpAclReferenceSet as usize, plan);
            }
        }
        geo.visit_geo_plugins(owner, |leaf| {
            if !leaf.is_supported() {
                return;
            }
            match compile_geo_leaf(&leaf) {
                Ok(predicate) => {
                    compiler.required.push(predicate.clone());
                    compiler
                        .bindings
                        .insert(leaf.config() as *const IpReferenceSet as usize, predicate);
                }
                Err(value) => error = Some(value),
            }
        })?;
        if let Some(error) = error {
            return Err(error);
        }
        Ok(compiler)
    }
    pub(crate) fn from_gateway_config(owner: &'owner EdgionGatewayConfig) -> Result<Self, ReferenceError> {
        let prepared = PreparedConditionReferences::from_gateway_config(owner);
        Self::from_visitor(|visitor| prepared.visit_gateway_ip(owner, visitor))
    }
    fn from_visitor(
        visit: impl FnOnce(&mut dyn for<'leaf> FnMut(BoundIpAccess<'leaf>)) -> Result<(), ReferenceError>,
    ) -> Result<Self, ReferenceError> {
        let mut bindings = HashMap::new();
        let mut required = Vec::new();
        let mut error = None;
        visit(&mut |leaf| {
            if !leaf.is_supported() {
                return;
            }
            match Self::compile_leaf(leaf) {
                Ok((node, predicate)) => {
                    if predicate.required() {
                        required.push(predicate.clone());
                    }
                    bindings.insert(node, predicate);
                }
                Err(value) => error = Some(value),
            }
        })?;
        if let Some(error) = error {
            return Err(error);
        }
        Ok(Self {
            bindings,
            required,
            acls: HashMap::new(),
            _owner: PhantomData,
        })
    }
    fn compile_leaf(leaf: BoundIpAccess<'_>) -> Result<(usize, Arc<BoundIpPredicate>), ReferenceError> {
        leaf.validate_owner()?;
        let config = leaf.config();
        let proof = leaf.read_scopes()?;
        let role = leaf.execution_role();
        let mut invalid = config.validate().is_err();
        if invalid && role == IpExecutionRole::Trust {
            return Err(ReferenceError::UnsupportedTarget);
        }
        let keys = compile_keys(
            proof.as_deref(),
            role,
            leaf.references()?
                .zip(&config.ip_refs)
                .map(|(token, reference)| (token, reference.ref_denied.is_some())),
            &mut invalid,
        )?;
        Ok((
            config as *const IpReferenceSet as usize,
            Arc::new(BoundIpPredicate { keys, role, invalid }),
        ))
    }
    pub(crate) fn bind(&self, refs: &IpReferenceSet) -> Option<Arc<BoundIpPredicate>> {
        self.bindings.get(&(refs as *const IpReferenceSet as usize)).cloned()
    }
    pub(crate) fn bind_acl(&self, refs: &IpAclReferenceSet) -> Option<Arc<CompiledIpAcl>> {
        self.acls.get(&(refs as *const IpAclReferenceSet as usize)).cloned()
    }
    pub(crate) fn has_refs(&self) -> bool {
        !self.bindings.is_empty() || !self.acls.is_empty()
    }
    pub(crate) fn required(&self) -> Vec<Arc<BoundIpPredicate>> {
        self.required.clone()
    }
}

pub(crate) fn compile_keys<'a>(
    proof: Option<&[IpReadScope]>,
    role: IpExecutionRole,
    tokens: impl Iterator<Item = (ReferenceToken<'a>, bool)>,
    invalid: &mut bool,
) -> Result<Vec<IpListReadKey>, ReferenceError> {
    let mut keys = Vec::new();
    for (index, (token, denied)) in tokens.enumerate() {
        match token.validated() {
            Ok(bound) => {
                let scope = proof.map(|scopes| scopes[index]).unwrap_or_else(|| {
                    if bound.is_same_namespace() {
                        IpReadScope::SameNamespace
                    } else {
                        IpReadScope::ClusterOnly
                    }
                });
                keys.push(IpListReadKey {
                    key: format!(
                        "{}/{}",
                        bound.target().scope().namespace().unwrap_or_default(),
                        bound.target().name()
                    ),
                    scope: if denied { IpReadScope::Denied } else { scope },
                });
            }
            Err(error) if role == IpExecutionRole::Trust => return Err(error),
            Err(_) => *invalid |= role != IpExecutionRole::Partial,
        }
    }
    Ok(keys)
}
pub(crate) fn compile_geo_leaf(leaf: &BoundGeoIpAccess<'_>) -> Result<Arc<BoundIpPredicate>, ReferenceError> {
    leaf.validate_owner()?;
    if !leaf.is_supported() || leaf.config().validate().is_err() {
        return Err(ReferenceError::UnsupportedTarget);
    }
    let proof = leaf.read_scopes()?;
    let mut invalid = false;
    let keys = compile_keys(
        proof.as_deref(),
        IpExecutionRole::Required,
        leaf.references()?
            .zip(&leaf.config().ip_refs)
            .map(|(token, reference)| (token, reference.ref_denied.is_some())),
        &mut invalid,
    )?;
    if invalid {
        return Err(ReferenceError::UnsupportedTarget);
    }
    Ok(Arc::new(BoundIpPredicate {
        keys,
        role: IpExecutionRole::Required,
        invalid,
    }))
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/runtime/conditions/ip_refs_test.rs"]
mod tests;
