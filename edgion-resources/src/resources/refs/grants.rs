//! Exact ReferenceGrant matching over a concrete, validated reference query.

use super::{ObjectIdentity, ReferenceSource, ResourceScope};
use crate::resources::reference_grant::ReferenceGrant;

/// Source and concrete target shared by permission and authority-revision checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GrantQuery<'a> {
    source: ReferenceSource<'a>,
    target: ObjectIdentity<'a>,
}

impl<'a> GrantQuery<'a> {
    pub(super) fn new(source: ReferenceSource<'a>, target: ObjectIdentity<'a>) -> Self {
        Self { source, target }
    }

    pub fn source(self) -> ReferenceSource<'a> {
        self.source
    }

    pub fn target(self) -> ObjectIdentity<'a> {
        self.target
    }

    /// Cluster authority and same-namespace exceptions are runtime policy,
    /// outside the matching relationship represented by a ReferenceGrant.
    pub fn matches(self, grant: &ReferenceGrant) -> bool {
        let ResourceScope::Namespaced(source_namespace) = self.source.scope() else {
            return false;
        };
        let ResourceScope::Namespaced(target_namespace) = self.target.scope() else {
            return false;
        };
        if grant.namespace() != Some(target_namespace) {
            return false;
        }
        let source = self.source.group_kind();
        let target = self.target.group_kind();
        grant.spec.from.iter().any(|entry| {
            entry.namespace == source_namespace && entry.group == source.group() && entry.kind == source.kind()
        }) && grant.spec.to.iter().any(|entry| {
            entry.group == target.group()
                && entry.kind == target.kind()
                && entry.name.as_deref().is_none_or(|name| name == self.target.name())
        })
    }
}

/// Validate the concrete query used by the typed ReferenceGrant convenience method.
/// The query's target is concrete; only a Grant's omitted target name is a wildcard.
pub(crate) fn allows_reference(
    grant: &ReferenceGrant,
    from_namespace: &str,
    from_group: &str,
    from_kind: &str,
    to_group: &str,
    to_kind: &str,
    to_name: Option<&str>,
) -> bool {
    let (Some(namespace), Some(name)) = (grant.namespace(), to_name) else {
        return false;
    };
    let Ok(source) = ReferenceSource::checked(from_group, from_kind, ResourceScope::Namespaced(from_namespace)) else {
        return false;
    };
    let Ok(target) = ObjectIdentity::checked(to_group, to_kind, ResourceScope::Namespaced(namespace), name) else {
        return false;
    };
    GrantQuery::new(source, target).matches(grant)
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/grants_test.rs"]
mod tests;
