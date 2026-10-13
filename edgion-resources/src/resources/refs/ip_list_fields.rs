//! Synchronous owning loans for standalone HTTP and Stream IpList fields.
use super::config_data_fields::{ConfigDataField, StreamConfigDataEntry};
use super::owned::{FieldContract, FieldSlot, FieldWire, OwnerSnapshot, PreparedSlot, TopologyLoan};
use super::{ReferenceError, ReferenceToken};
use crate::resources::edgion_config_data::EdgionConfigDataRef;
use crate::resources::edgion_plugins::ip_acl_refs::IpAclReferenceSet;
use crate::resources::edgion_plugins::ip_reference_set::{IpReadDomain, IpReadScope, IpReferenceSet};
use crate::resources::edgion_plugins::plugin_configs::{GeoIpLocationConfig, GeoRuleKey, GeoRuleSource};
use crate::resources::edgion_plugins::PluginValidation;
use crate::resources::edgion_plugins::{EdgionPlugin, EdgionPlugins, IpRestrictionConfig};
use crate::resources::edgion_stream_plugins::{
    ConnectionIpRestrictionRules, EdgionStreamPlugin, EdgionStreamPlugins, GlobalConnectionIpRestrictionConfig,
    TlsRouteStreamPlugin,
};

#[derive(Clone, PartialEq, Eq)]
enum Site {
    Http(u8, usize),
    Connection(usize),
    Profile(usize, String),
    Tls(usize),
    GeoHttp(u8, usize, bool, usize, usize),
    GeoStream(usize, bool, usize, usize),
}
#[derive(Clone, PartialEq, Eq)]
struct GeoShape {
    key: GeoRuleKey,
    source: GeoRuleSource,
    config_invalid: bool,
    group_invalid: bool,
    rule_invalid: bool,
    has_values: bool,
}
struct Root<'a> {
    site: Site,
    shape: Option<GeoShape>,
    acl_state: Option<(bool, bool)>,
    refs: [Option<&'a Vec<EdgionConfigDataRef>>; 2],
}
struct PreparedRoot {
    index: usize,
    site: Site,
    shape: Option<GeoShape>,
    acl_state: Option<(bool, bool)>,
    refs: [Option<Vec<PreparedSlot>>; 2],
}
pub(super) struct IpListSnapshot {
    pub(super) owner: OwnerSnapshot,
    headers: Vec<(u8, usize, bool, String)>,
    roots: Vec<PreparedRoot>,
}
/// Prepared descriptors never own the resource or compiled matchers.
///
/// Loans cannot outlive the actual owner or prepared topology.
/// ```compile_fail
/// use edgion_resources::refs::ip_list_fields::PreparedIpListReferences;
/// let token = {
///     let owner: edgion_resources::EdgionPlugins = serde_json::from_value(serde_json::json!({
///         "metadata":{"name":"owner"},"spec":{"requestPlugins":[{"type":"IpRestriction","config":{"allowRefs":[{"name":"nets"}]}}]}
///     })).unwrap();
///     let prepared = PreparedIpListReferences::from_plugins(&owner);
///     let binding = prepared.bind_plugins(&owner).unwrap();
///     let entry = binding.http_entry(0).unwrap();
///     let refs = entry.references();
///     let token = refs.allow().next().unwrap().0;
///     token
/// };
/// let _ = token.validated();
/// ```
pub struct PreparedIpListReferences {
    snapshot: IpListSnapshot,
}
fn wire(reference: &EdgionConfigDataRef) -> FieldWire {
    FieldWire::ConfigData {
        name: reference.name.clone(),
        namespace: reference.namespace.clone(),
    }
}
impl PreparedIpListReferences {
    pub fn from_plugins(owner: &EdgionPlugins) -> Self {
        Self::new(
            OwnerSnapshot::namespaced("edgion.io", "EdgionPlugins", &owner.metadata),
            http_headers(owner),
            http_roots(owner),
        )
    }
    pub fn from_stream_plugins(owner: &EdgionStreamPlugins) -> Self {
        Self::new(
            OwnerSnapshot::namespaced("edgion.io", "EdgionStreamPlugins", &owner.metadata),
            stream_headers(owner),
            stream_roots(owner),
        )
    }
    fn new(owner: OwnerSnapshot, headers: Vec<(u8, usize, bool, String)>, roots: Vec<Root<'_>>) -> Self {
        let roots = roots
            .into_iter()
            .enumerate()
            .map(|(root, value)| {
                let geo = matches!(value.site, Site::GeoHttp(..) | Site::GeoStream(..));
                PreparedRoot {
                    index: root,
                    acl_state: value.acl_state,
                    shape: value.shape,
                    site: value.site.clone(),
                    refs: std::array::from_fn(|side| {
                        value.refs[side].map(|refs| {
                            refs.iter()
                                .enumerate()
                                .map(|(reference, value)| {
                                    PreparedSlot::new(
                                        &owner,
                                        wire(value),
                                        FieldContract::FixedConfigData,
                                        if geo {
                                            FieldSlot::GeoIp { root, reference }
                                        } else {
                                            FieldSlot::IpList {
                                                root,
                                                reference,
                                                deny: side == 1,
                                            }
                                        },
                                    )
                                })
                                .collect()
                        })
                    }),
                }
            })
            .collect();
        Self {
            snapshot: IpListSnapshot { owner, headers, roots },
        }
    }

    fn check(
        &self,
        metadata: &kube::api::ObjectMeta,
        headers: Vec<(u8, usize, bool, String)>,
        roots: &[Root<'_>],
    ) -> Result<(), ReferenceError> {
        if !self.snapshot.owner.matches(metadata)
            || self.snapshot.headers != headers
            || roots.len() != self.snapshot.roots.len()
        {
            return Err(ReferenceError::StaleBinding);
        }
        for (root, prepared) in roots.iter().zip(&self.snapshot.roots) {
            if root.site != prepared.site || root.shape != prepared.shape || root.acl_state != prepared.acl_state {
                return Err(ReferenceError::StaleBinding);
            }
            for (actual, slots) in root.refs.iter().zip(&prepared.refs) {
                let matches = match (actual, slots) {
                    (None, None) => true,
                    (Some(actual), Some(slots)) => {
                        actual.len() == slots.len() && actual.iter().zip(slots).all(|(r, s)| wire(r) == s.wire)
                    }
                    _ => false,
                };
                if !matches {
                    return Err(ReferenceError::StaleBinding);
                }
            }
        }
        Ok(())
    }
    pub fn bind_plugins<'a>(&'a self, owner: &'a EdgionPlugins) -> Result<HttpIpListBindings<'a>, ReferenceError> {
        self.check(&owner.metadata, http_headers(owner), &http_roots(owner))?;
        Ok(HttpIpListBindings { prepared: self, owner })
    }
    pub fn bind_stream_plugins<'a>(
        &'a self,
        owner: &'a EdgionStreamPlugins,
    ) -> Result<StreamIpListBindings<'a>, ReferenceError> {
        self.check(&owner.metadata, stream_headers(owner), &stream_roots(owner))?;
        Ok(StreamIpListBindings { prepared: self, owner })
    }
    fn refs<'a>(&'a self, site: &Site, config: &'a IpAclReferenceSet) -> Option<BoundIpListRefs<'a>> {
        let root = self.snapshot.roots.iter().find(|root| &root.site == site)?;
        Some(BoundIpListRefs {
            root,
            config,
            refs: [config.allow_refs.as_ref(), config.deny_refs.as_ref()],
            snapshot: &self.snapshot,
        })
    }
}
#[derive(Clone, Copy)]
pub struct BoundIpListRefs<'a> {
    config: &'a IpAclReferenceSet,
    root: &'a PreparedRoot,
    refs: [Option<&'a Vec<EdgionConfigDataRef>>; 2],
    snapshot: &'a IpListSnapshot,
}
impl<'a> BoundIpListRefs<'a> {
    pub fn validate_owner(&self) -> Result<(), ReferenceError> {
        if self.refs.iter().flatten().any(|refs| !refs.is_empty()) {
            self.snapshot.owner.source()?;
        }
        Ok(())
    }
    fn side(&self, side: usize) -> impl Iterator<Item = (ReferenceToken<'a>, bool)> + '_ {
        self.root.refs[side]
            .iter()
            .flatten()
            .zip(self.refs[side].into_iter().flatten())
            .map(|(slot, reference)| {
                (
                    slot.token(TopologyLoan::IpList(self.snapshot)),
                    reference.ref_denied.is_some(),
                )
            })
    }
    pub fn allow(&self) -> impl Iterator<Item = (ReferenceToken<'a>, bool)> + '_ {
        self.side(0)
    }
    pub fn deny(&self) -> impl Iterator<Item = (ReferenceToken<'a>, bool)> + '_ {
        self.side(1)
    }
}
pub struct HttpIpListBindings<'a> {
    prepared: &'a PreparedIpListReferences,
    owner: &'a EdgionPlugins,
}
impl<'a> HttpIpListBindings<'a> {
    pub fn http_entry(&self, index: usize) -> Option<HttpIpListEntry<'a>> {
        let EdgionPlugin::IpRestriction(config) = &self.owner.spec.request_plugins.as_ref()?.get(index)?.plugin else {
            return None;
        };
        Some(HttpIpListEntry {
            config,
            refs: self.prepared.refs(&Site::Http(0, index), &config.refs)?,
        })
    }
    pub fn references(&self) -> impl Iterator<Item = BoundIpListRefs<'a>> + '_ {
        self.prepared.snapshot.roots.iter().filter_map(|root| match root.site {
            Site::Http(0, index) => self.http_entry(index).map(|entry| entry.refs),
            _ => None,
        })
    }
}
pub struct StreamIpListBindings<'a> {
    prepared: &'a PreparedIpListReferences,
    owner: &'a EdgionStreamPlugins,
}
impl<'a> StreamIpListBindings<'a> {
    pub fn owner(&self) -> &'a EdgionStreamPlugins {
        self.owner
    }
    pub fn connection_entry(&self, index: usize) -> Option<ConnectionIpListEntry<'a>> {
        let EdgionStreamPlugin::IpRestriction(config) = &self.owner.spec.plugins.as_ref()?.get(index)?.plugin else {
            return None;
        };
        Some(ConnectionIpListEntry {
            config,
            refs: self.prepared.refs(&Site::Connection(index), &config.refs)?,
        })
    }
    pub fn tls_entry(&self, index: usize) -> Option<HttpIpListEntry<'a>> {
        let TlsRouteStreamPlugin::IpRestriction(config) =
            &self.owner.spec.tls_route_plugins.as_ref()?.get(index)?.plugin
        else {
            return None;
        };
        Some(HttpIpListEntry {
            config,
            refs: self.prepared.refs(&Site::Tls(index), &config.refs)?,
        })
    }
    pub fn global_entry(&self, index: usize) -> Option<GlobalIpListEntry<'a>> {
        let EdgionStreamPlugin::GlobalConnectionIpRestriction(config) =
            &self.owner.spec.plugins.as_ref()?.get(index)?.plugin
        else {
            return None;
        };
        Some(GlobalIpListEntry {
            owner: self.owner,
            config,
            index,
            prepared: self.prepared,
        })
    }
    pub fn references(&self) -> impl Iterator<Item = BoundIpListRefs<'a>> + '_ {
        self.prepared.snapshot.roots.iter().filter_map(|root| match &root.site {
            Site::Connection(index) => self.connection_entry(*index).map(|entry| entry.refs),
            Site::Tls(index) => self.tls_entry(*index).map(|entry| entry.refs),
            Site::Profile(index, name) => {
                let entry = self.global_entry(*index)?;
                let config = entry.config.profiles.get(name)?;
                self.prepared.refs(&root.site, &config.refs)
            }
            Site::Http(..) | Site::GeoHttp(..) | Site::GeoStream(..) => None,
        })
    }
}
pub struct HttpIpListEntry<'a> {
    config: &'a IpRestrictionConfig,
    refs: BoundIpListRefs<'a>,
}
impl<'a> HttpIpListEntry<'a> {
    pub fn config(&self) -> &'a IpRestrictionConfig {
        self.config
    }
    pub fn references(&self) -> BoundIpListRefs<'a> {
        self.refs
    }
}
pub struct ConnectionIpListEntry<'a> {
    config: &'a ConnectionIpRestrictionRules,
    refs: BoundIpListRefs<'a>,
}
impl<'a> ConnectionIpListEntry<'a> {
    pub fn config(&self) -> &'a ConnectionIpRestrictionRules {
        self.config
    }
    pub fn references(&self) -> BoundIpListRefs<'a> {
        self.refs
    }
}
pub struct GlobalIpListEntry<'a> {
    owner: &'a EdgionStreamPlugins,
    config: &'a GlobalConnectionIpRestrictionConfig,
    index: usize,
    prepared: &'a PreparedIpListReferences,
}
impl<'a> GlobalIpListEntry<'a> {
    pub fn selector(&self) -> Option<ConfigDataField<'a>> {
        StreamConfigDataEntry::from_stream_plugins(self.owner, self.index)?.field()
    }
    pub fn config(&self) -> &'a GlobalConnectionIpRestrictionConfig {
        self.config
    }
    pub fn namespace(&self) -> &str {
        self.prepared.snapshot.owner.namespace().unwrap_or("default")
    }
    pub fn profiles(&self) -> impl Iterator<Item = (&'a str, BoundIpListRefs<'a>)> + '_ {
        self.prepared.snapshot.roots.iter().filter_map(move |root| {
            let Site::Profile(index, name) = &root.site else {
                return None;
            };
            if *index != self.index {
                return None;
            }
            let profile = self.config.profiles.get(name)?;
            Some((
                name.as_str(),
                BoundIpListRefs {
                    root,
                    config: &profile.refs,
                    snapshot: &self.prepared.snapshot,
                    refs: [profile.refs.allow_refs.as_ref(), profile.refs.deny_refs.as_ref()],
                },
            ))
        })
    }
}
fn http_stages(owner: &EdgionPlugins) -> [Vec<(&EdgionPlugin, bool)>; 4] {
    macro_rules! stage {
        ($field:ident) => {
            owner
                .spec
                .$field
                .iter()
                .flatten()
                .map(|entry| (&entry.plugin, entry.enable))
                .collect()
        };
    }
    [
        stage!(request_plugins),
        stage!(upstream_response_filter_plugins),
        stage!(upstream_response_body_filter_plugins),
        stage!(upstream_response_plugins),
    ]
}
fn http_headers(owner: &EdgionPlugins) -> Vec<(u8, usize, bool, String)> {
    let presence = [
        owner.spec.request_plugins.is_some(),
        owner.spec.upstream_response_filter_plugins.is_some(),
        owner.spec.upstream_response_body_filter_plugins.is_some(),
        owner.spec.upstream_response_plugins.is_some(),
    ];
    let mut headers = Vec::new();
    for (stage, entries) in http_stages(owner).into_iter().enumerate() {
        headers.push((stage as u8, entries.len(), presence[stage], String::new()));
        headers.extend(
            entries
                .into_iter()
                .enumerate()
                .map(|(index, (plugin, enable))| (stage as u8, index, enable, plugin.type_name().into())),
        );
    }
    headers
}
fn stream_headers(owner: &EdgionStreamPlugins) -> Vec<(u8, usize, bool, String)> {
    let mut headers = vec![
        (
            0,
            owner.spec.plugins.as_ref().map_or(0, Vec::len),
            owner.spec.plugins.is_some(),
            String::new(),
        ),
        (
            1,
            owner.spec.tls_route_plugins.as_ref().map_or(0, Vec::len),
            owner.spec.tls_route_plugins.is_some(),
            String::new(),
        ),
    ];
    headers.extend(
        owner
            .spec
            .plugins
            .iter()
            .flatten()
            .enumerate()
            .map(|(index, entry)| (0, index, entry.enable, entry.type_name().into())),
    );
    headers.extend(
        owner
            .spec
            .tls_route_plugins
            .iter()
            .flatten()
            .enumerate()
            .map(|(index, entry)| (1, index, entry.enable, entry.type_name().into())),
    );
    headers
}
fn http_roots(owner: &EdgionPlugins) -> Vec<Root<'_>> {
    let mut roots = Vec::new();
    for (stage, entries) in http_stages(owner).into_iter().enumerate() {
        for (index, (plugin, enabled)) in entries.into_iter().enumerate() {
            if let EdgionPlugin::IpRestriction(config) = plugin {
                roots.push(Root {
                    shape: None,
                    acl_state: Some((config.validation_error().is_some(), enabled && stage == 0)),
                    site: Site::Http(stage as u8, index),
                    refs: [config.refs.allow_refs.as_ref(), config.refs.deny_refs.as_ref()],
                });
            }
        }
    }
    for (stage, entries) in http_stages(owner).into_iter().enumerate() {
        for (index, (plugin, _)) in entries.into_iter().enumerate() {
            if let EdgionPlugin::GeoIpLocation(config) = plugin {
                add_geo_roots(&mut roots, config, |deny, group, rule| {
                    Site::GeoHttp(stage as u8, index, deny, group, rule)
                });
            }
        }
    }
    roots
}
fn stream_roots(owner: &EdgionStreamPlugins) -> Vec<Root<'_>> {
    let mut roots = Vec::new();
    for (index, entry) in owner.spec.plugins.iter().flatten().enumerate() {
        match &entry.plugin {
            EdgionStreamPlugin::IpRestriction(config) => roots.push(Root {
                shape: None,
                site: Site::Connection(index),
                acl_state: Some((config.validation_error().is_some(), entry.enable)),
                refs: [config.refs.allow_refs.as_ref(), config.refs.deny_refs.as_ref()],
            }),
            EdgionStreamPlugin::GlobalConnectionIpRestriction(config) => {
                let mut names: Vec<_> = config.profiles.keys().collect();
                names.sort();
                for name in names {
                    let profile = &config.profiles[name];
                    roots.push(Root {
                        shape: None,
                        site: Site::Profile(index, name.clone()),
                        acl_state: Some((config.validation_error().is_some(), entry.enable && config.enable)),
                        refs: [profile.refs.allow_refs.as_ref(), profile.refs.deny_refs.as_ref()],
                    });
                }
            }
            EdgionStreamPlugin::GeoIpLocation(config) => add_geo_roots(&mut roots, config, |deny, group, rule| {
                Site::GeoStream(index, deny, group, rule)
            }),
            _ => {}
        }
    }
    for (index, entry) in owner.spec.tls_route_plugins.iter().flatten().enumerate() {
        if let TlsRouteStreamPlugin::IpRestriction(config) = &entry.plugin {
            roots.push(Root {
                shape: None,
                site: Site::Tls(index),
                acl_state: Some((config.validation_error().is_some(), entry.enable)),
                refs: [config.refs.allow_refs.as_ref(), config.refs.deny_refs.as_ref()],
            });
        }
    }
    roots
}
fn add_geo_roots<'a>(
    roots: &mut Vec<Root<'a>>,
    config: &'a GeoIpLocationConfig,
    site: impl Fn(bool, usize, usize) -> Site,
) {
    for (deny, groups) in [(true, &config.deny_rule_groups), (false, &config.allow_rule_groups)] {
        for (group, value) in groups.iter().enumerate() {
            for (rule, value_rule) in value.rules.iter().enumerate() {
                roots.push(Root {
                    site: site(deny, group, rule),
                    acl_state: None,
                    shape: Some(GeoShape {
                        key: value_rule.key,
                        source: value_rule.source,
                        config_invalid: config.invalid_input,
                        group_invalid: value.invalid_input,
                        rule_invalid: value_rule.cidr.invalid_input,
                        has_values: !value_rule.values.is_empty(),
                    }),
                    refs: [Some(&value_rule.cidr.ip_refs), None],
                });
            }
        }
    }
}

fn geo_rule(
    config: &GeoIpLocationConfig,
    deny: bool,
    group: usize,
    rule: usize,
) -> Option<&crate::resources::edgion_plugins::plugin_configs::GeoRule> {
    (if deny {
        &config.deny_rule_groups
    } else {
        &config.allow_rule_groups
    })
    .get(group)?
    .rules
    .get(rule)
}
fn geo_rule_mut(
    config: &mut GeoIpLocationConfig,
    deny: bool,
    group: usize,
    rule: usize,
) -> Option<&mut crate::resources::edgion_plugins::plugin_configs::GeoRule> {
    (if deny {
        &mut config.deny_rule_groups
    } else {
        &mut config.allow_rule_groups
    })
    .get_mut(group)?
    .rules
    .get_mut(rule)
}
fn http_geo(owner: &EdgionPlugins, stage: u8, index: usize) -> Option<&GeoIpLocationConfig> {
    let plugin = match stage {
        0 => &owner.spec.request_plugins.as_ref()?.get(index)?.plugin,
        1 => &owner.spec.upstream_response_filter_plugins.as_ref()?.get(index)?.plugin,
        2 => {
            &owner
                .spec
                .upstream_response_body_filter_plugins
                .as_ref()?
                .get(index)?
                .plugin
        }
        3 => &owner.spec.upstream_response_plugins.as_ref()?.get(index)?.plugin,
        _ => return None,
    };
    let EdgionPlugin::GeoIpLocation(config) = plugin else {
        return None;
    };
    Some(config)
}
fn http_geo_mut(owner: &mut EdgionPlugins, stage: u8, index: usize) -> Option<&mut GeoIpLocationConfig> {
    let plugin = match stage {
        0 => &mut owner.spec.request_plugins.as_mut()?.get_mut(index)?.plugin,
        1 => {
            &mut owner
                .spec
                .upstream_response_filter_plugins
                .as_mut()?
                .get_mut(index)?
                .plugin
        }
        2 => {
            &mut owner
                .spec
                .upstream_response_body_filter_plugins
                .as_mut()?
                .get_mut(index)?
                .plugin
        }
        3 => &mut owner.spec.upstream_response_plugins.as_mut()?.get_mut(index)?.plugin,
        _ => return None,
    };
    let EdgionPlugin::GeoIpLocation(config) = plugin else {
        return None;
    };
    Some(config)
}
pub struct BoundGeoIpAccess<'a> {
    config: &'a IpReferenceSet,
    root: &'a PreparedRoot,
    snapshot: &'a IpListSnapshot,
    supported: bool,
    coordinates: (bool, usize, usize),
}
pub struct BoundGeoIpAccessMut<'a> {
    config: &'a mut IpReferenceSet,
    root: &'a PreparedRoot,
    snapshot: &'a IpListSnapshot,
    supported: bool,
    coordinates: (bool, usize, usize),
}
macro_rules! geo_access {
    () => {
        pub fn config(&self) -> &IpReferenceSet {
            self.config
        }
        pub fn coordinates(&self) -> (bool, usize, usize) {
            self.coordinates
        }
        pub fn is_supported(&self) -> bool {
            self.supported
        }
        pub fn validate_owner(&self) -> Result<(), ReferenceError> {
            self.snapshot.owner.source()?;
            if self
                .root
                .shape
                .as_ref()
                .is_none_or(|shape| shape.rule_invalid != self.config.invalid_input)
            {
                return Err(ReferenceError::StaleBinding);
            }
            let slots = self.root.refs[0].as_ref().ok_or(ReferenceError::StaleBinding)?;
            if self.config.ip_refs.len() != slots.len()
                || !self
                    .config
                    .ip_refs
                    .iter()
                    .zip(slots)
                    .all(|(value, slot)| wire(value) == slot.wire)
            {
                return Err(ReferenceError::StaleBinding);
            }
            Ok(())
        }
        pub fn references(&self) -> Result<impl Iterator<Item = ReferenceToken<'_>>, ReferenceError> {
            self.validate_owner()?;
            Ok(self.root.refs[0]
                .iter()
                .flatten()
                .map(|slot| slot.token(TopologyLoan::IpList(self.snapshot))))
        }
        fn resolution(
            &self,
            scopes: &[IpReadScope],
        ) -> Result<crate::resources::edgion_plugins::ip_reference_set::IpReadResolution, ReferenceError> {
            self.validate_owner()?;
            let root = self
                .snapshot
                .roots
                .iter()
                .position(|root| std::ptr::eq(root, self.root))
                .ok_or(ReferenceError::StaleBinding)?;
            super::owned::ip_read_resolution(
                &self.snapshot.owner,
                IpReadDomain::IpList,
                root,
                None,
                self.references()?,
                scopes,
            )
        }
        pub fn read_scopes(&self) -> Result<Option<Vec<IpReadScope>>, ReferenceError> {
            let Some(proof) = self.config.resolved_refs.as_deref() else {
                return Ok(None);
            };
            let scopes: Vec<_> = proof.slots.iter().map(|slot| slot.scope).collect();
            if self.resolution(&scopes)? != *proof {
                return Err(ReferenceError::StaleBinding);
            }
            Ok(Some(scopes))
        }
    };
}
impl BoundGeoIpAccess<'_> {
    geo_access!();
}
impl BoundGeoIpAccessMut<'_> {
    geo_access!();
    pub fn config_mut(&mut self) -> &mut IpReferenceSet {
        self.config
    }
    pub fn set_read_scopes(&mut self, scopes: &[IpReadScope]) -> Result<(), ReferenceError> {
        let proof = self.resolution(scopes)?;
        self.config.resolved_refs = Some(Box::new(proof));
        Ok(())
    }
}
pub struct BoundGeoIpEntry<'a> {
    config: &'a GeoIpLocationConfig,
    leaves: Vec<BoundGeoIpAccess<'a>>,
}
impl<'a> BoundGeoIpEntry<'a> {
    pub fn config(&self) -> &'a GeoIpLocationConfig {
        self.config
    }
    pub fn leaves(&self) -> impl Iterator<Item = &BoundGeoIpAccess<'a>> {
        self.leaves.iter()
    }
}
impl PreparedIpListReferences {
    pub fn visit_geo_plugins(
        &self,
        owner: &EdgionPlugins,
        mut visit: impl for<'a> FnMut(BoundGeoIpAccess<'a>),
    ) -> Result<(), ReferenceError> {
        self.check(&owner.metadata, http_headers(owner), &http_roots(owner))?;
        for root in &self.snapshot.roots {
            let Site::GeoHttp(stage, index, deny, group, rule) = root.site else {
                continue;
            };
            let config = http_geo(owner, stage, index).ok_or(ReferenceError::StaleBinding)?;
            let value = geo_rule(config, deny, group, rule).ok_or(ReferenceError::StaleBinding)?;
            let active = http_stages(owner)[stage as usize][index].1;
            visit(BoundGeoIpAccess {
                config: &value.cidr,
                root,
                snapshot: &self.snapshot,
                supported: stage == 0
                    && active
                    && config.check_fields(false).is_none()
                    && value.key == GeoRuleKey::Cidr,
                coordinates: (deny, group, rule),
            });
        }
        Ok(())
    }
    pub fn visit_geo_plugins_mut(
        &self,
        owner: &mut EdgionPlugins,
        mut visit: impl for<'a> FnMut(BoundGeoIpAccessMut<'a>),
    ) -> Result<(), ReferenceError> {
        self.check(&owner.metadata, http_headers(owner), &http_roots(owner))?;
        for root in &self.snapshot.roots {
            let Site::GeoHttp(stage, index, deny, group, rule) = root.site else {
                continue;
            };
            let active = http_stages(owner)[stage as usize][index].1;
            let config = http_geo_mut(owner, stage, index).ok_or(ReferenceError::StaleBinding)?;
            let supported = stage == 0 && active && config.check_fields(false).is_none();
            let value = geo_rule_mut(config, deny, group, rule).ok_or(ReferenceError::StaleBinding)?;
            visit(BoundGeoIpAccessMut {
                supported: supported && value.key == GeoRuleKey::Cidr,
                config: &mut value.cidr,
                root,
                snapshot: &self.snapshot,
                coordinates: (deny, group, rule),
            });
        }
        Ok(())
    }
    pub fn visit_geo_stream_plugins(
        &self,
        owner: &EdgionStreamPlugins,
        mut visit: impl for<'a> FnMut(BoundGeoIpAccess<'a>),
    ) -> Result<(), ReferenceError> {
        self.check(&owner.metadata, stream_headers(owner), &stream_roots(owner))?;
        for root in &self.snapshot.roots {
            let Site::GeoStream(index, deny, group, rule) = root.site else {
                continue;
            };
            let entry = &owner.spec.plugins.as_ref().ok_or(ReferenceError::StaleBinding)?[index];
            let EdgionStreamPlugin::GeoIpLocation(config) = &entry.plugin else {
                return Err(ReferenceError::StaleBinding);
            };
            let value = geo_rule(config, deny, group, rule).ok_or(ReferenceError::StaleBinding)?;
            visit(BoundGeoIpAccess {
                config: &value.cidr,
                root,
                snapshot: &self.snapshot,
                supported: entry.enable && config.check_fields(true).is_none() && value.key == GeoRuleKey::Cidr,
                coordinates: (deny, group, rule),
            });
        }
        Ok(())
    }
    pub fn visit_geo_stream_plugins_mut(
        &self,
        owner: &mut EdgionStreamPlugins,
        mut visit: impl for<'a> FnMut(BoundGeoIpAccessMut<'a>),
    ) -> Result<(), ReferenceError> {
        self.check(&owner.metadata, stream_headers(owner), &stream_roots(owner))?;
        for root in &self.snapshot.roots {
            let Site::GeoStream(index, deny, group, rule) = root.site else {
                continue;
            };
            let entry = &mut owner.spec.plugins.as_mut().ok_or(ReferenceError::StaleBinding)?[index];
            let EdgionStreamPlugin::GeoIpLocation(config) = &mut entry.plugin else {
                return Err(ReferenceError::StaleBinding);
            };
            let supported = entry.enable && config.check_fields(true).is_none();
            let value = geo_rule_mut(config, deny, group, rule).ok_or(ReferenceError::StaleBinding)?;
            visit(BoundGeoIpAccessMut {
                supported: supported && value.key == GeoRuleKey::Cidr,
                config: &mut value.cidr,
                root,
                snapshot: &self.snapshot,
                coordinates: (deny, group, rule),
            });
        }
        Ok(())
    }
}
impl<'a> HttpIpListBindings<'a> {
    pub fn geo_entry(&self, index: usize) -> Option<BoundGeoIpEntry<'a>> {
        let config = http_geo(self.owner, 0, index)?;
        let mut leaves = Vec::new();
        for root in &self.prepared.snapshot.roots {
            let Site::GeoHttp(0, current, deny, group, rule) = root.site else {
                continue;
            };
            if current != index {
                continue;
            }
            let value = geo_rule(config, deny, group, rule)?;
            leaves.push(BoundGeoIpAccess {
                config: &value.cidr,
                root,
                snapshot: &self.prepared.snapshot,
                supported: self.owner.spec.request_plugins.as_ref()?[index].enable
                    && config.check_fields(false).is_none()
                    && value.key == GeoRuleKey::Cidr,
                coordinates: (deny, group, rule),
            });
        }
        Some(BoundGeoIpEntry { config, leaves })
    }
}
impl<'a> StreamIpListBindings<'a> {
    pub fn geo_entry(&self, index: usize) -> Option<BoundGeoIpEntry<'a>> {
        let entry = self.owner.spec.plugins.as_ref()?.get(index)?;
        let EdgionStreamPlugin::GeoIpLocation(config) = &entry.plugin else {
            return None;
        };
        let mut leaves = Vec::new();
        for root in &self.prepared.snapshot.roots {
            let Site::GeoStream(current, deny, group, rule) = root.site else {
                continue;
            };
            if current != index {
                continue;
            }
            let value = geo_rule(config, deny, group, rule)?;
            leaves.push(BoundGeoIpAccess {
                config: &value.cidr,
                root,
                snapshot: &self.prepared.snapshot,
                supported: entry.enable && config.check_fields(true).is_none() && value.key == GeoRuleKey::Cidr,
                coordinates: (deny, group, rule),
            });
        }
        Some(BoundGeoIpEntry { config, leaves })
    }
}
#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/ip_list_fields_test.rs"]
mod tests;

/// Side identity is part of the owning proof; allow and deny are not interchangeable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpListSide {
    Allow,
    Deny,
}
impl IpListSide {
    fn index(self) -> usize {
        match self {
            Self::Allow => 0,
            Self::Deny => 1,
        }
    }
}
impl<'a> BoundIpListRefs<'a> {
    pub fn config(&self) -> &'a IpAclReferenceSet {
        self.config
    }
    pub fn is_supported(&self) -> bool {
        self.root
            .acl_state
            .is_some_and(|(invalid, enabled)| !invalid && enabled)
    }
    /// Original standalone Controller observation includes disabled policy fields.
    /// This reports a catalogue site only; it confers no executable authority.
    pub fn is_controller_reference_site(&self) -> bool {
        matches!(
            self.root.site,
            Site::Http(0, _) | Site::Connection(_) | Site::Profile(_, _) | Site::Tls(_)
        )
    }
    pub fn references(&self, side: IpListSide) -> impl Iterator<Item = (ReferenceToken<'a>, bool)> + '_ {
        self.side(side.index())
    }
    fn resolution(
        &self,
        side: IpListSide,
        scopes: &[IpReadScope],
    ) -> Result<crate::resources::edgion_plugins::ip_reference_set::IpReadResolution, ReferenceError> {
        self.validate_owner()?;
        super::owned::ip_read_resolution(
            &self.snapshot.owner,
            IpReadDomain::IpList,
            self.root.index,
            Some((side.index(), None)),
            self.references(side).map(|(token, _)| token),
            scopes,
        )
    }
    pub fn read_scopes(&self, side: IpListSide) -> Result<Option<Vec<IpReadScope>>, ReferenceError> {
        self.validate_owner()?;
        let stored = match side {
            IpListSide::Allow => self.config.resolved_allow_refs.as_deref(),
            IpListSide::Deny => self.config.resolved_deny_refs.as_deref(),
        };
        let Some(stored) = stored else {
            return Ok(None);
        };
        let scopes: Vec<_> = stored.slots.iter().map(|slot| slot.scope).collect();
        if *stored != self.resolution(side, &scopes)? {
            return Err(ReferenceError::StaleBinding);
        }
        Ok(Some(scopes))
    }
}
pub struct BoundIpListRefsMut<'a> {
    config: &'a mut IpAclReferenceSet,
    root: &'a PreparedRoot,
    snapshot: &'a IpListSnapshot,
}
impl BoundIpListRefsMut<'_> {
    fn borrowed(&self) -> BoundIpListRefs<'_> {
        BoundIpListRefs {
            config: self.config,
            root: self.root,
            snapshot: self.snapshot,
            refs: [self.config.allow_refs.as_ref(), self.config.deny_refs.as_ref()],
        }
    }
    pub fn is_supported(&self) -> bool {
        self.borrowed().is_supported()
    }
    pub fn clear_read_metadata(&mut self) {
        self.config.resolved_allow_refs = None;
        self.config.resolved_deny_refs = None;
        for reference in self
            .config
            .allow_refs
            .iter_mut()
            .chain(&mut self.config.deny_refs)
            .flatten()
        {
            reference.ref_denied = None;
        }
    }
    pub fn references(&self, side: IpListSide) -> impl Iterator<Item = (ReferenceToken<'_>, bool)> {
        let index = side.index();
        let refs = match side {
            IpListSide::Allow => &self.config.allow_refs,
            IpListSide::Deny => &self.config.deny_refs,
        };
        self.root.refs[index]
            .iter()
            .flatten()
            .zip(refs.iter().flatten())
            .map(|(slot, reference)| {
                (
                    slot.token(TopologyLoan::IpList(self.snapshot)),
                    reference.ref_denied.is_some(),
                )
            })
    }
    pub fn set_read_scopes(&mut self, side: IpListSide, scopes: &[IpReadScope]) -> Result<(), ReferenceError> {
        let proof = self.borrowed().resolution(side, scopes)?;
        let target = match side {
            IpListSide::Allow => &mut self.config.resolved_allow_refs,
            IpListSide::Deny => &mut self.config.resolved_deny_refs,
        };
        *target = Some(Box::new(proof));
        Ok(())
    }
    pub fn set_denied(
        &mut self,
        side: IpListSide,
        denied: Vec<Option<crate::resources::common::RefDenied>>,
    ) -> Result<(), ReferenceError> {
        let refs = match side {
            IpListSide::Allow => &mut self.config.allow_refs,
            IpListSide::Deny => &mut self.config.deny_refs,
        };
        if refs.as_ref().map_or(0, Vec::len) != denied.len() {
            return Err(ReferenceError::StaleBinding);
        }
        for (reference, denied) in refs.iter_mut().flatten().zip(denied) {
            reference.ref_denied = denied;
        }
        Ok(())
    }
}
fn http_acl(owner: &EdgionPlugins, stage: u8, index: usize) -> Option<&IpAclReferenceSet> {
    let plugin = match stage {
        0 => &owner.spec.request_plugins.as_ref()?.get(index)?.plugin,
        1 => &owner.spec.upstream_response_filter_plugins.as_ref()?.get(index)?.plugin,
        2 => {
            &owner
                .spec
                .upstream_response_body_filter_plugins
                .as_ref()?
                .get(index)?
                .plugin
        }
        3 => &owner.spec.upstream_response_plugins.as_ref()?.get(index)?.plugin,
        _ => return None,
    };
    let EdgionPlugin::IpRestriction(config) = plugin else {
        return None;
    };
    Some(&config.refs)
}
fn http_acl_mut(owner: &mut EdgionPlugins, stage: u8, index: usize) -> Option<&mut IpAclReferenceSet> {
    let plugin = match stage {
        0 => &mut owner.spec.request_plugins.as_mut()?.get_mut(index)?.plugin,
        1 => {
            &mut owner
                .spec
                .upstream_response_filter_plugins
                .as_mut()?
                .get_mut(index)?
                .plugin
        }
        2 => {
            &mut owner
                .spec
                .upstream_response_body_filter_plugins
                .as_mut()?
                .get_mut(index)?
                .plugin
        }
        3 => &mut owner.spec.upstream_response_plugins.as_mut()?.get_mut(index)?.plugin,
        _ => return None,
    };
    let EdgionPlugin::IpRestriction(config) = plugin else {
        return None;
    };
    Some(&mut config.refs)
}
fn stream_acl<'a>(owner: &'a EdgionStreamPlugins, site: &Site) -> Option<&'a IpAclReferenceSet> {
    match site {
        Site::Connection(index) => {
            let EdgionStreamPlugin::IpRestriction(config) = &owner.spec.plugins.as_ref()?.get(*index)?.plugin else {
                return None;
            };
            Some(&config.refs)
        }
        Site::Profile(index, name) => {
            let EdgionStreamPlugin::GlobalConnectionIpRestriction(config) =
                &owner.spec.plugins.as_ref()?.get(*index)?.plugin
            else {
                return None;
            };
            Some(&config.profiles.get(name)?.refs)
        }
        Site::Tls(index) => {
            let TlsRouteStreamPlugin::IpRestriction(config) =
                &owner.spec.tls_route_plugins.as_ref()?.get(*index)?.plugin
            else {
                return None;
            };
            Some(&config.refs)
        }
        _ => None,
    }
}
fn stream_acl_mut<'a>(owner: &'a mut EdgionStreamPlugins, site: &Site) -> Option<&'a mut IpAclReferenceSet> {
    match site {
        Site::Connection(index) => {
            let EdgionStreamPlugin::IpRestriction(config) = &mut owner.spec.plugins.as_mut()?.get_mut(*index)?.plugin
            else {
                return None;
            };
            Some(&mut config.refs)
        }
        Site::Profile(index, name) => {
            let EdgionStreamPlugin::GlobalConnectionIpRestriction(config) =
                &mut owner.spec.plugins.as_mut()?.get_mut(*index)?.plugin
            else {
                return None;
            };
            Some(&mut config.profiles.get_mut(name)?.refs)
        }
        Site::Tls(index) => {
            let TlsRouteStreamPlugin::IpRestriction(config) =
                &mut owner.spec.tls_route_plugins.as_mut()?.get_mut(*index)?.plugin
            else {
                return None;
            };
            Some(&mut config.refs)
        }
        _ => None,
    }
}
impl PreparedIpListReferences {
    pub fn visit_acl_plugins(
        &self,
        owner: &EdgionPlugins,
        mut visit: impl for<'a> FnMut(BoundIpListRefs<'a>),
    ) -> Result<(), ReferenceError> {
        self.check(&owner.metadata, http_headers(owner), &http_roots(owner))?;
        for root in &self.snapshot.roots {
            let Site::Http(stage, index) = root.site else {
                continue;
            };
            visit(
                self.refs(
                    &root.site,
                    http_acl(owner, stage, index).ok_or(ReferenceError::StaleBinding)?,
                )
                .ok_or(ReferenceError::StaleBinding)?,
            );
        }
        Ok(())
    }
    pub fn visit_acl_plugins_mut(
        &self,
        owner: &mut EdgionPlugins,
        mut visit: impl for<'a> FnMut(BoundIpListRefsMut<'a>),
    ) -> Result<(), ReferenceError> {
        self.check(&owner.metadata, http_headers(owner), &http_roots(owner))?;
        for root in &self.snapshot.roots {
            let Site::Http(stage, index) = root.site else {
                continue;
            };
            visit(BoundIpListRefsMut {
                config: http_acl_mut(owner, stage, index).ok_or(ReferenceError::StaleBinding)?,
                root,
                snapshot: &self.snapshot,
            });
        }
        Ok(())
    }
    pub fn visit_acl_stream_plugins(
        &self,
        owner: &EdgionStreamPlugins,
        mut visit: impl for<'a> FnMut(BoundIpListRefs<'a>),
    ) -> Result<(), ReferenceError> {
        self.check(&owner.metadata, stream_headers(owner), &stream_roots(owner))?;
        for root in &self.snapshot.roots {
            if let Some(config) = stream_acl(owner, &root.site) {
                visit(self.refs(&root.site, config).ok_or(ReferenceError::StaleBinding)?);
            }
        }
        Ok(())
    }
    pub fn visit_acl_stream_plugins_mut(
        &self,
        owner: &mut EdgionStreamPlugins,
        mut visit: impl for<'a> FnMut(BoundIpListRefsMut<'a>),
    ) -> Result<(), ReferenceError> {
        self.check(&owner.metadata, stream_headers(owner), &stream_roots(owner))?;
        for root in &self.snapshot.roots {
            if let Some(config) = stream_acl_mut(owner, &root.site) {
                visit(BoundIpListRefsMut {
                    config,
                    root,
                    snapshot: &self.snapshot,
                });
            }
        }
        Ok(())
    }
}
