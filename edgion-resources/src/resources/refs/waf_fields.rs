//! Synchronous WAF field loans. Transport validation alone carries no Grant authority.
use super::{GrantQuery, ObjectIdentity, ReferenceError, ReferenceSource, ResourceScope};
use crate::resources::edgion_config_data::{ConfigEntry, EdgionConfigDataRef, WafBundleRef, WafPolicyData};
use crate::{EdgionConfigData, EdgionPlugins};

fn owner_identity<'a>(
    kind: &'a str,
    namespace: Option<&'a str>,
    name: Option<&'a str>,
) -> Result<ObjectIdentity<'a>, ReferenceError> {
    ObjectIdentity::checked(
        "edgion.io",
        kind,
        ResourceScope::Namespaced(namespace.unwrap_or("default")),
        name.unwrap_or(""),
    )
}

fn target_identity<'a>(
    reference: &'a EdgionConfigDataRef,
    namespace: &'a str,
) -> Result<ObjectIdentity<'a>, ReferenceError> {
    ObjectIdentity::checked(
        "edgion.io",
        "EdgionConfigData",
        ResourceScope::Namespaced(reference.namespace.as_deref().unwrap_or(namespace)),
        &reference.name,
    )
}

/// Lexical validation of a selected transport descriptor, not a resource-owner loan.
pub fn validated_bundle_target<'a>(
    reference: &'a WafBundleRef,
    policy_namespace: &'a str,
) -> Result<ObjectIdentity<'a>, ReferenceError> {
    ReferenceSource::checked(
        "edgion.io",
        "EdgionConfigData",
        ResourceScope::Namespaced(policy_namespace),
    )?;
    target_identity(&reference.target, policy_namespace)
}

/// A validated field whose lifetime is bounded by its actual resource owner.
pub struct WafReference<'a> {
    query: GrantQuery<'a>,
}
impl WafReference<'_> {
    pub fn query(&self) -> GrantQuery<'_> {
        self.query
    }
    pub fn target(&self) -> ObjectIdentity<'_> {
        self.query.target()
    }
    pub fn key(&self) -> String {
        let ResourceScope::Namespaced(namespace) = self.target().scope() else {
            unreachable!()
        };
        format!("{namespace}/{}", self.target().name())
    }
}

fn reference<'a>(
    owner: ObjectIdentity<'a>,
    field: &'a EdgionConfigDataRef,
) -> Result<WafReference<'a>, ReferenceError> {
    let ResourceScope::Namespaced(namespace) = owner.scope() else {
        unreachable!()
    };
    let source = ReferenceSource::checked(owner.group_kind().group(), owner.group_kind().kind(), owner.scope())?;
    Ok(WafReference {
        query: GrantQuery::new(source, target_identity(field, namespace)?),
    })
}

/// Policy and Selector fields borrow the actual EdgionPlugins, never a projected config.
/// ```compile_fail
/// use edgion_resources::{EdgionPlugins, resources::refs::waf_fields::WafPluginReferences};
/// fn escape(owner: EdgionPlugins) {
///     let fields = WafPluginReferences::from_plugins(&owner).unwrap();
///     let field = fields.policy().unwrap();
///     drop(owner);
///     field.key();
/// }
/// ```
pub struct WafPluginReferences<'a> {
    owner: &'a EdgionPlugins,
    identity: ObjectIdentity<'a>,
}
impl<'a> WafPluginReferences<'a> {
    pub fn from_plugins(owner: &'a EdgionPlugins) -> Result<Self, ReferenceError> {
        let identity = owner_identity(
            "EdgionPlugins",
            owner.metadata.namespace.as_deref(),
            owner.metadata.name.as_deref(),
        )?;
        if owner.spec.waf.is_none() {
            return Err(ReferenceError::UnsupportedTarget);
        }
        Ok(Self { owner, identity })
    }
    pub fn policy(&self) -> Result<WafReference<'a>, ReferenceError> {
        reference(
            self.identity,
            &self.owner.spec.waf.as_ref().expect("checked WAF owner").policy_ref,
        )
    }
    pub fn selector(&self) -> Option<Result<WafReference<'a>, ReferenceError>> {
        self.owner
            .spec
            .waf
            .as_ref()
            .expect("checked WAF owner")
            .active_profile_ref
            .as_ref()
            .map(|field| reference(self.identity, field))
    }
}

/// Bundle fields borrow the captured ConfigData containing the actual WafPolicy.
/// ```compile_fail
/// use edgion_resources::{EdgionConfigData, resources::refs::waf_fields::WafPolicyReferences};
/// fn escape(owner: EdgionConfigData) {
///     let fields = WafPolicyReferences::from_policy(&owner).unwrap();
///     let field = fields.bundle("default", 0).unwrap().unwrap();
///     drop(owner);
///     field.key();
/// }
/// ```
pub struct WafPolicyReferences<'a> {
    policy: &'a WafPolicyData,
    identity: ObjectIdentity<'a>,
}
impl<'a> WafPolicyReferences<'a> {
    pub fn from_policy(owner: &'a EdgionConfigData) -> Result<Self, ReferenceError> {
        let identity = owner_identity(
            "EdgionConfigData",
            owner.metadata.namespace.as_deref(),
            owner.metadata.name.as_deref(),
        )?;
        let ConfigEntry::WafPolicy(policy) = &owner.spec.data else {
            return Err(ReferenceError::UnsupportedTarget);
        };
        Ok(Self { policy, identity })
    }
    pub fn identity(&self) -> ObjectIdentity<'a> {
        self.identity
    }
    pub fn bundle(&self, profile: &str, index: usize) -> Option<Result<WafReference<'a>, ReferenceError>> {
        let field = self.policy.profiles.get(profile)?.bundle_refs.get(index)?;
        Some(reference(self.identity, &field.target))
    }
    pub fn fields(
        &self,
    ) -> impl Iterator<
        Item = (
            &'a str,
            usize,
            &'a WafBundleRef,
            Result<WafReference<'a>, ReferenceError>,
        ),
    > + '_ {
        self.policy.profiles.iter().flat_map(move |(name, profile)| {
            profile
                .bundle_refs
                .iter()
                .enumerate()
                .map(move |(index, field)| (name.as_str(), index, field, reference(self.identity, &field.target)))
        })
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/waf_fields_test.rs"]
mod tests;
