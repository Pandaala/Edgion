//! Typed dependency keys for Gateway frontend CA sources.

use super::{BidirectionalRefManager, ResourceRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrontendCaSourceKind {
    Secret,
    ConfigMap,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FrontendCaSourceKey {
    pub kind: FrontendCaSourceKind,
    pub namespace: String,
    pub name: String,
}

impl FrontendCaSourceKey {
    pub fn new(kind: FrontendCaSourceKind, namespace: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            kind,
            namespace: namespace.into(),
            name: name.into(),
        }
    }

    pub fn index_key(&self) -> String {
        let kind = match self.kind {
            FrontendCaSourceKind::Secret => "Secret",
            FrontendCaSourceKind::ConfigMap => "ConfigMap",
        };
        format!("{kind}:{}/{}", self.namespace, self.name)
    }
}

pub type FrontendCaRefManager = BidirectionalRefManager<ResourceRef>;
