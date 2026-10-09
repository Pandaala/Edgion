//! Borrowed Kubernetes identities without runtime policy or material.

use crate::resources::common::kubernetes_name::{is_dns_label, is_dns_subdomain};

/// An invalid authored identity, independent of lookup or authorization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceError {
    InvalidGroup,
    InvalidKind,
    InvalidName,
    InvalidNamespace,
    MissingNamespace,
    UnsupportedTarget,
    InvalidPort,
    StaleBinding,
}

impl std::fmt::Display for ReferenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidGroup => "invalid reference group",
            Self::InvalidKind => "invalid reference kind",
            Self::InvalidName => "invalid reference name",
            Self::InvalidNamespace => "invalid reference namespace",
            Self::MissingNamespace => "reference requires an explicit namespace",
            Self::UnsupportedTarget => "unsupported reference target",
            Self::InvalidPort => "invalid reference port",
            Self::StaleBinding => "reference binding no longer matches its owning field",
        })
    }
}

impl std::error::Error for ReferenceError {}

/// Exact API group and kind. The empty group is the Kubernetes core group.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GroupKind<'a> {
    group: &'a str,
    kind: &'a str,
}

impl<'a> GroupKind<'a> {
    pub(super) fn checked(group: &'a str, kind: &'a str) -> Result<Self, ReferenceError> {
        if !group.is_empty() && !is_dns_subdomain(group, 253) {
            return Err(ReferenceError::InvalidGroup);
        }
        let bytes = kind.as_bytes();
        if bytes.is_empty()
            || bytes.len() > 63
            || !bytes[0].is_ascii_alphabetic()
            || !bytes.last().is_some_and(u8::is_ascii_alphanumeric)
            || !bytes.iter().all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
        {
            return Err(ReferenceError::InvalidKind);
        }
        Ok(Self { group, kind })
    }

    pub fn group(self) -> &'a str {
        self.group
    }

    pub fn kind(self) -> &'a str {
        self.kind
    }
}

/// Scope is explicit; a cluster resource never acquires a fictitious namespace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResourceScope<'a> {
    Cluster,
    Namespaced(&'a str),
}

/// The type and real scope of an authorizing source; Grant sources have no name selector.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ReferenceSource<'a> {
    group_kind: GroupKind<'a>,
    scope: ResourceScope<'a>,
}

impl<'a> ReferenceSource<'a> {
    pub(super) fn checked(group: &'a str, kind: &'a str, scope: ResourceScope<'a>) -> Result<Self, ReferenceError> {
        let group_kind = GroupKind::checked(group, kind)?;
        if let ResourceScope::Namespaced(namespace) = scope {
            if !is_dns_label(namespace, 63) {
                return Err(ReferenceError::InvalidNamespace);
            }
        }
        Ok(Self { group_kind, scope })
    }

    pub fn group_kind(self) -> GroupKind<'a> {
        self.group_kind
    }

    pub fn scope(self) -> ResourceScope<'a> {
        self.scope
    }
}

impl<'a> ResourceScope<'a> {
    pub fn namespace(self) -> Option<&'a str> {
        match self {
            Self::Cluster => None,
            Self::Namespaced(namespace) => Some(namespace),
        }
    }
}

/// Validated borrowed object identity, constructed only by owning field adapters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ObjectIdentity<'a> {
    group_kind: GroupKind<'a>,
    scope: ResourceScope<'a>,
    name: &'a str,
}

impl<'a> ObjectIdentity<'a> {
    /// Fixed DNS annotation grammar; only its private owning adapter selects it.
    pub(super) fn parsed_dns_linksys(namespace: &'a str, name: &'a str) -> Self {
        Self {
            group_kind: GroupKind {
                group: "edgion.io",
                kind: "LinkSys",
            },
            scope: ResourceScope::Namespaced(namespace),
            name,
        }
    }

    /// Fixed template read grammar, selected only by private owning adapters.
    /// Explicit nonempty components retain the canonical parser's lexical contract.
    pub(super) fn parsed_template_secret(namespace: &'a str, name: &'a str) -> Result<Self, ReferenceError> {
        if namespace.is_empty() {
            return Err(ReferenceError::InvalidNamespace);
        }
        if name.is_empty() {
            return Err(ReferenceError::InvalidName);
        }
        Ok(Self {
            group_kind: GroupKind::checked("", "Secret")?,
            scope: ResourceScope::Namespaced(namespace),
            name,
        })
    }
    /// Native Gateway API ObjectName uses a character-count bound, not DNS syntax.
    pub(super) fn native_object_reference(
        group: &'a str,
        kind: &'a str,
        scope: ResourceScope<'a>,
        name: &'a str,
    ) -> Result<Self, ReferenceError> {
        let group_kind = GroupKind::checked(group, kind)?;
        if let ResourceScope::Namespaced(namespace) = scope {
            if !is_dns_label(namespace, 63) {
                return Err(ReferenceError::InvalidNamespace);
            }
        }
        if name.is_empty() || name.chars().count() > 253 {
            return Err(ReferenceError::InvalidName);
        }
        Ok(Self {
            group_kind,
            scope,
            name,
        })
    }

    pub(super) fn checked(
        group: &'a str,
        kind: &'a str,
        scope: ResourceScope<'a>,
        name: &'a str,
    ) -> Result<Self, ReferenceError> {
        let group_kind = GroupKind::checked(group, kind)?;
        if let ResourceScope::Namespaced(namespace) = scope {
            if !is_dns_label(namespace, 63) {
                return Err(ReferenceError::InvalidNamespace);
            }
        }
        if !is_dns_subdomain(name, 253) {
            return Err(ReferenceError::InvalidName);
        }
        Ok(Self {
            group_kind,
            scope,
            name,
        })
    }

    pub fn group_kind(self) -> GroupKind<'a> {
        self.group_kind
    }

    pub fn scope(self) -> ResourceScope<'a> {
        self.scope
    }

    pub fn name(self) -> &'a str {
        self.name
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/identity_test.rs"]
mod tests;
