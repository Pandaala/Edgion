//! Canonical parent attachment and controller-owned status identities.

use super::ParentReference;
use crate::resources::http_route::{RouteParentStatus, GATEWAY_API_GROUP};

/// Borrowed attachment identity, independent of its representation in YAML.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ParentRefIdentity<'a> {
    pub group: &'a str,
    pub kind: &'a str,
    pub namespace: &'a str,
    pub name: &'a str,
    pub section_name: Option<&'a str>,
    pub port: Option<i32>,
}

impl ParentRefIdentity<'_> {
    pub fn is_gateway(&self) -> bool {
        self.group == GATEWAY_API_GROUP && self.kind == "Gateway"
    }
}

/// A status belongs to both an attachment and the controller that evaluated it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ParentStatusIdentity<'a> {
    pub parent: ParentRefIdentity<'a>,
    pub controller_name: &'a str,
}

impl ParentReference {
    /// Resolve Gateway API defaults using the namespace of the referring resource.
    /// An explicit empty group remains distinct from an omitted Gateway API group.
    pub fn canonical_identity<'a>(&'a self, route_namespace: Option<&'a str>) -> ParentRefIdentity<'a> {
        ParentRefIdentity {
            group: self.group.as_deref().unwrap_or(GATEWAY_API_GROUP),
            kind: self.kind.as_deref().unwrap_or("Gateway"),
            namespace: self.namespace.as_deref().or(route_namespace).unwrap_or("default"),
            name: &self.name,
            section_name: self.section_name.as_deref(),
            port: self.port,
        }
    }
}

impl RouteParentStatus {
    pub fn canonical_identity<'a>(&'a self, route_namespace: Option<&'a str>) -> ParentStatusIdentity<'a> {
        ParentStatusIdentity {
            parent: self.parent_ref.canonical_identity(route_namespace),
            controller_name: &self.controller_name,
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/common/parent_identity_test.rs"]
mod tests;
