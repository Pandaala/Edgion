//! Finite synchronous loans for plugin Selector, Region and URL-document fields.
use super::{GrantQuery, ObjectIdentity, ReferenceError, ReferenceSource, ResourceScope};
use crate::resources::edgion_config_data::{ConfigEntry, EdgionConfigDataRef};
use crate::resources::edgion_plugins::plugin_configs::{RequestAccessPolicyConfig, RequestAccessRuleType};
use crate::resources::edgion_plugins::EdgionPlugin;
use crate::resources::edgion_stream_plugins::{EdgionStreamPlugin, GlobalConnectionIpRestrictionConfig};
use crate::{EdgionPlugins, EdgionStreamPlugins};

#[derive(Clone, Copy)]
enum Owner<'a> {
    Http(&'a EdgionPlugins),
    Stream(&'a EdgionStreamPlugins),
}
impl Owner<'_> {
    fn identity(&self) -> Result<ObjectIdentity<'_>, ReferenceError> {
        let (kind, metadata) = match self {
            Self::Http(owner) => ("EdgionPlugins", &owner.metadata),
            Self::Stream(owner) => ("EdgionStreamPlugins", &owner.metadata),
        };
        ObjectIdentity::checked(
            "edgion.io",
            kind,
            ResourceScope::Namespaced(metadata.namespace.as_deref().unwrap_or("default")),
            metadata.name.as_deref().unwrap_or(""),
        )
    }
}

/// Material roles are selected by the actual owning variant, not by a caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigDataRole {
    Selector,
    RegionOverride,
    UrlAllowList,
}
impl ConfigDataRole {
    pub fn expected_type(self) -> &'static str {
        match self {
            Self::Selector => "Selector",
            Self::RegionOverride => "RegionRouteOverride",
            Self::UrlAllowList => "RequestAccessUrlAllowList",
        }
    }
    pub fn matches(self, data: &ConfigEntry) -> bool {
        matches!(
            (self, data),
            (Self::Selector, ConfigEntry::Selector(_))
                | (Self::RegionOverride, ConfigEntry::RegionRouteOverride(_))
                | (Self::UrlAllowList, ConfigEntry::RequestAccessUrlAllowList(_))
        )
    }
}

pub struct ConfigDataField<'a> {
    owner: Owner<'a>,
    reference: &'a EdgionConfigDataRef,
    role: ConfigDataRole,
}
impl ConfigDataField<'_> {
    pub fn role(&self) -> ConfigDataRole {
        self.role
    }
    pub fn validate_owner(&self) -> Result<(), ReferenceError> {
        self.owner.identity().map(|_| ())
    }
    pub fn query(&self) -> Result<GrantQuery<'_>, ReferenceError> {
        let owner = self.owner.identity()?;
        let namespace = self
            .reference
            .namespace
            .as_deref()
            .or(owner.scope().namespace())
            .ok_or(ReferenceError::MissingNamespace)?;
        let target = ObjectIdentity::checked(
            "edgion.io",
            "EdgionConfigData",
            ResourceScope::Namespaced(namespace),
            &self.reference.name,
        )?;
        let source = ReferenceSource::checked(owner.group_kind().group(), owner.group_kind().kind(), owner.scope())?;
        Ok(GrantQuery::new(source, target))
    }
    pub fn key(&self) -> Result<String, ReferenceError> {
        let target = self.query()?.target();
        Ok(format!(
            "{}/{}",
            target.scope().namespace().expect("namespaced target"),
            target.name()
        ))
    }
    pub fn denied(&self) -> bool {
        self.reference.ref_denied.is_some()
    }
    /// Only usable identities may become cold runtime keys. Consumer fallback is separate.
    pub fn runtime_key(&self) -> Result<Option<String>, ReferenceError> {
        let key = self.key()?;
        Ok((!self.denied()).then_some(key))
    }
}

/// The actual request entry bounds every field loan, including authored URL slots.
/// ```compile_fail
/// use edgion_resources::{EdgionPlugins, refs::config_data_fields::HttpConfigDataEntry};
/// fn escape(owner: EdgionPlugins) {
///     let entry = HttpConfigDataEntry::from_plugins(&owner, 0).unwrap();
///     let field = entry.field().unwrap();
///     drop(owner);
///     field.key();
/// }
/// ```
#[derive(Clone, Copy)]
pub struct HttpConfigDataEntry<'a> {
    owner: &'a EdgionPlugins,
    index: usize,
}
impl<'a> HttpConfigDataEntry<'a> {
    pub fn from_plugins(owner: &'a EdgionPlugins, index: usize) -> Option<Self> {
        match &owner.spec.request_plugins.as_ref()?.get(index)?.plugin {
            EdgionPlugin::RequestAccessPolicy(_) | EdgionPlugin::Canary(_) | EdgionPlugin::RegionRoute(_) => {
                Some(Self { owner, index })
            }
            _ => None,
        }
    }
    pub fn plugin(&self) -> &'a EdgionPlugin {
        &self.owner.spec.request_plugins.as_ref().expect("bound entry")[self.index].plugin
    }
    pub fn validate_owner(&self) -> Result<(), ReferenceError> {
        Owner::Http(self.owner).identity().map(|_| ())
    }
    pub fn field(&self) -> Option<ConfigDataField<'a>> {
        let (reference, role) = match self.plugin() {
            EdgionPlugin::RequestAccessPolicy(config) => {
                (config.active_profile_ref.as_ref()?, ConfigDataRole::Selector)
            }
            EdgionPlugin::Canary(config) => (config.active_profile_ref.as_ref()?, ConfigDataRole::Selector),
            EdgionPlugin::RegionRoute(config) => (config.override_ref.as_ref()?, ConfigDataRole::RegionOverride),
            _ => unreachable!("bound variant"),
        };
        Some(ConfigDataField {
            owner: Owner::Http(self.owner),
            reference,
            role,
        })
    }
    pub fn url_field(&self, profile: &str, group: usize, rule: usize, reference: usize) -> Option<ConfigDataField<'a>> {
        let EdgionPlugin::RequestAccessPolicy(config) = self.plugin() else {
            return None;
        };
        let rule = config
            .profiles
            .get(profile)?
            .required_rule_groups
            .get(group)?
            .any_of_rules
            .get(rule)?;
        let RequestAccessRuleType::Url(config) = &rule.rule else {
            return None;
        };
        Some(ConfigDataField {
            owner: Owner::Http(self.owner),
            reference: &config.config_refs.get(reference)?.reference,
            role: ConfigDataRole::UrlAllowList,
        })
    }
    pub fn has_references(&self) -> bool {
        if self.field().is_some() {
            return true;
        }
        let EdgionPlugin::RequestAccessPolicy(config) = self.plugin() else {
            return false;
        };
        request_access_has_external_data(config)
    }
}

/// GCIR Selector authority is the actual ESP, independently of profile IpList loans.
/// ```compile_fail
/// use edgion_resources::{EdgionStreamPlugins, refs::config_data_fields::StreamConfigDataEntry};
/// fn escape(owner: EdgionStreamPlugins) {
///     let entry = StreamConfigDataEntry::from_stream_plugins(&owner, 0).unwrap();
///     let field = entry.field().unwrap();
///     drop(owner);
///     field.key();
/// }
/// ```
#[derive(Clone, Copy)]
pub struct StreamConfigDataEntry<'a> {
    owner: &'a EdgionStreamPlugins,
    index: usize,
}
impl<'a> StreamConfigDataEntry<'a> {
    pub fn from_stream_plugins(owner: &'a EdgionStreamPlugins, index: usize) -> Option<Self> {
        matches!(
            &owner.spec.plugins.as_ref()?.get(index)?.plugin,
            EdgionStreamPlugin::GlobalConnectionIpRestriction(_)
        )
        .then_some(Self { owner, index })
    }
    pub fn config(&self) -> &'a GlobalConnectionIpRestrictionConfig {
        let EdgionStreamPlugin::GlobalConnectionIpRestriction(config) =
            &self.owner.spec.plugins.as_ref().expect("bound entry")[self.index].plugin
        else {
            unreachable!("bound variant")
        };
        config
    }
    pub fn field(&self) -> Option<ConfigDataField<'a>> {
        Some(ConfigDataField {
            owner: Owner::Stream(self.owner),
            reference: self.config().active_profile_ref.as_ref()?,
            role: ConfigDataRole::Selector,
        })
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/config_data_fields_test.rs"]
mod tests;

/// Reference presence is a shape predicate, not permission or lookup authority.
pub fn request_access_has_external_data(config: &RequestAccessPolicyConfig) -> bool {
    config.active_profile_ref.is_some()
        || config
            .profiles
            .values()
            .flat_map(|profile| &profile.required_rule_groups)
            .flat_map(|group| &group.any_of_rules)
            .any(|rule| matches!(&rule.rule, RequestAccessRuleType::Url(config) if !config.config_refs.is_empty()))
}
