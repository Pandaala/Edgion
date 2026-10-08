//! Synchronous owning loans for standalone HTTP and Stream IpList fields.
use super::config_data_fields::{ConfigDataField, StreamConfigDataEntry};
use super::owned::{FieldContract, FieldSlot, FieldWire, OwnerSnapshot, PreparedSlot, TopologyLoan};
use super::{ReferenceError, ReferenceToken};
use crate::resources::common::RefDenied;
use crate::resources::edgion_config_data::EdgionConfigDataRef;
use crate::resources::edgion_plugins::{EdgionPlugin, EdgionPlugins, IpRestrictionConfig};
use crate::resources::edgion_stream_plugins::{
    ConnectionIpRestrictionRules, EdgionStreamPlugin, EdgionStreamPlugins, GlobalConnectionIpRestrictionConfig,
    TlsRouteStreamPlugin,
};

#[derive(Clone, PartialEq, Eq)]
enum Site {
    Http(usize),
    Connection(usize),
    Profile(usize, String),
    Tls(usize),
}
struct Root<'a> {
    site: Site,
    refs: [Option<&'a Vec<EdgionConfigDataRef>>; 2],
}
struct PreparedRoot {
    site: Site,
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
            .map(|(root, value)| PreparedRoot {
                site: value.site,
                refs: std::array::from_fn(|side| {
                    value.refs[side].map(|refs| {
                        refs.iter()
                            .enumerate()
                            .map(|(reference, value)| {
                                PreparedSlot::new(
                                    &owner,
                                    wire(value),
                                    FieldContract::FixedConfigData,
                                    FieldSlot::IpList {
                                        root,
                                        reference,
                                        deny: side == 1,
                                    },
                                )
                            })
                            .collect()
                    })
                }),
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
            if root.site != prepared.site {
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
    fn refs<'a>(&'a self, site: &Site, refs: [Option<&'a Vec<EdgionConfigDataRef>>; 2]) -> Option<BoundIpListRefs<'a>> {
        let root = self.snapshot.roots.iter().find(|root| &root.site == site)?;
        Some(BoundIpListRefs {
            root,
            refs,
            snapshot: &self.snapshot,
        })
    }
    pub fn visit_plugins_mut(
        &self,
        owner: &mut EdgionPlugins,
        mut visit: impl for<'a> FnMut(BoundIpListRefMut<'a>),
    ) -> Result<(), ReferenceError> {
        self.check(&owner.metadata, http_headers(owner), &http_roots(owner))?;
        for root in &self.snapshot.roots {
            let Site::Http(index) = root.site else {
                return Err(ReferenceError::StaleBinding);
            };
            let EdgionPlugin::IpRestriction(config) = &mut owner
                .spec
                .request_plugins
                .as_mut()
                .ok_or(ReferenceError::StaleBinding)?[index]
                .plugin
            else {
                return Err(ReferenceError::StaleBinding);
            };
            visit_refs_mut(
                &self.snapshot,
                root,
                [config.allow_refs.as_mut(), config.deny_refs.as_mut()],
                &mut visit,
            );
        }
        Ok(())
    }
    pub fn visit_stream_plugins_mut(
        &self,
        owner: &mut EdgionStreamPlugins,
        mut visit: impl for<'a> FnMut(BoundIpListRefMut<'a>),
    ) -> Result<(), ReferenceError> {
        self.check(&owner.metadata, stream_headers(owner), &stream_roots(owner))?;
        for root in &self.snapshot.roots {
            let refs = match &root.site {
                Site::Connection(index) => {
                    let EdgionStreamPlugin::IpRestriction(config) =
                        &mut owner.spec.plugins.as_mut().ok_or(ReferenceError::StaleBinding)?[*index].plugin
                    else {
                        return Err(ReferenceError::StaleBinding);
                    };
                    [config.allow_refs.as_mut(), config.deny_refs.as_mut()]
                }
                Site::Profile(index, profile) => {
                    let EdgionStreamPlugin::GlobalConnectionIpRestriction(config) =
                        &mut owner.spec.plugins.as_mut().ok_or(ReferenceError::StaleBinding)?[*index].plugin
                    else {
                        return Err(ReferenceError::StaleBinding);
                    };
                    let config = config.profiles.get_mut(profile).ok_or(ReferenceError::StaleBinding)?;
                    [config.allow_refs.as_mut(), config.deny_refs.as_mut()]
                }
                Site::Tls(index) => {
                    let TlsRouteStreamPlugin::IpRestriction(config) = &mut owner
                        .spec
                        .tls_route_plugins
                        .as_mut()
                        .ok_or(ReferenceError::StaleBinding)?[*index]
                        .plugin
                    else {
                        return Err(ReferenceError::StaleBinding);
                    };
                    [config.allow_refs.as_mut(), config.deny_refs.as_mut()]
                }
                Site::Http(_) => return Err(ReferenceError::StaleBinding),
            };
            visit_refs_mut(&self.snapshot, root, refs, &mut visit);
        }
        Ok(())
    }
}
fn visit_refs_mut(
    snapshot: &IpListSnapshot,
    root: &PreparedRoot,
    refs: [Option<&mut Vec<EdgionConfigDataRef>>; 2],
    visit: &mut impl for<'a> FnMut(BoundIpListRefMut<'a>),
) {
    for (refs, slots) in refs.into_iter().zip(&root.refs) {
        for (reference, slot) in refs.into_iter().flatten().zip(slots.iter().flatten()) {
            visit(BoundIpListRefMut {
                reference,
                slot,
                snapshot,
            });
        }
    }
}
pub struct BoundIpListRefMut<'a> {
    reference: &'a mut EdgionConfigDataRef,
    slot: &'a PreparedSlot,
    snapshot: &'a IpListSnapshot,
}
impl BoundIpListRefMut<'_> {
    pub fn token(&self) -> ReferenceToken<'_> {
        self.slot.token(TopologyLoan::IpList(self.snapshot))
    }
    pub fn set_denied(&mut self, denied: Option<RefDenied>) {
        self.reference.ref_denied = denied;
    }
}
#[derive(Clone, Copy)]
pub struct BoundIpListRefs<'a> {
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
            refs: self.prepared.refs(
                &Site::Http(index),
                [config.allow_refs.as_ref(), config.deny_refs.as_ref()],
            )?,
        })
    }
    pub fn references(&self) -> impl Iterator<Item = BoundIpListRefs<'a>> + '_ {
        self.prepared.snapshot.roots.iter().filter_map(|root| match root.site {
            Site::Http(index) => self.http_entry(index).map(|entry| entry.refs),
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
            refs: self.prepared.refs(
                &Site::Connection(index),
                [config.allow_refs.as_ref(), config.deny_refs.as_ref()],
            )?,
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
            refs: self.prepared.refs(
                &Site::Tls(index),
                [config.allow_refs.as_ref(), config.deny_refs.as_ref()],
            )?,
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
                self.prepared
                    .refs(&root.site, [config.allow_refs.as_ref(), config.deny_refs.as_ref()])
            }
            Site::Http(_) => None,
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
                    snapshot: &self.prepared.snapshot,
                    refs: [profile.allow_refs.as_ref(), profile.deny_refs.as_ref()],
                },
            ))
        })
    }
}
fn http_headers(owner: &EdgionPlugins) -> Vec<(u8, usize, bool, String)> {
    let mut headers = vec![(
        0,
        owner.spec.request_plugins.as_ref().map_or(0, Vec::len),
        owner.spec.request_plugins.is_some(),
        String::new(),
    )];
    headers.extend(
        owner
            .spec
            .request_plugins
            .iter()
            .flatten()
            .enumerate()
            .map(|(index, entry)| (0, index, entry.enable, entry.plugin.type_name().into())),
    );
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
    owner
        .spec
        .request_plugins
        .iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, entry)| {
            let EdgionPlugin::IpRestriction(config) = &entry.plugin else {
                return None;
            };
            Some(Root {
                site: Site::Http(index),
                refs: [config.allow_refs.as_ref(), config.deny_refs.as_ref()],
            })
        })
        .collect()
}
fn stream_roots(owner: &EdgionStreamPlugins) -> Vec<Root<'_>> {
    let mut roots = Vec::new();
    for (index, entry) in owner.spec.plugins.iter().flatten().enumerate() {
        match &entry.plugin {
            EdgionStreamPlugin::IpRestriction(config) => roots.push(Root {
                site: Site::Connection(index),
                refs: [config.allow_refs.as_ref(), config.deny_refs.as_ref()],
            }),
            EdgionStreamPlugin::GlobalConnectionIpRestriction(config) => {
                let mut names: Vec<_> = config.profiles.keys().collect();
                names.sort();
                for name in names {
                    let profile = &config.profiles[name];
                    roots.push(Root {
                        site: Site::Profile(index, name.clone()),
                        refs: [profile.allow_refs.as_ref(), profile.deny_refs.as_ref()],
                    });
                }
            }
            _ => {}
        }
    }
    for (index, entry) in owner.spec.tls_route_plugins.iter().flatten().enumerate() {
        if let TlsRouteStreamPlugin::IpRestriction(config) = &entry.plugin {
            roots.push(Root {
                site: Site::Tls(index),
                refs: [config.allow_refs.as_ref(), config.deny_refs.as_ref()],
            });
        }
    }
    roots
}
#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/ip_list_fields_test.rs"]
mod tests;
