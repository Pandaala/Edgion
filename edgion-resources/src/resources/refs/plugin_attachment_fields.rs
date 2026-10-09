//! Actual HTTP plugin attachment fields; permission and runtime failure stay with consumers.
use super::{ObjectIdentity, ReferenceError, ResourceScope};
use crate::constants::annotations::edgion::{parse_plugin_ref_list, EDGION_PLUGINS};
use crate::{EdgionGatewayConfig, Gateway};

/// A checked cold key. Only actual owning fields can construct this projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginAttachmentKey(String);
impl PluginAttachmentKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn into_key(self) -> String {
        self.0
    }
    fn checked(namespace: &str, name: &str) -> Result<Self, ReferenceError> {
        ObjectIdentity::checked("edgion.io", "EdgionPlugins", ResourceScope::Namespaced(namespace), name)?;
        Ok(Self(format!("{namespace}/{name}")))
    }
}

/// Borrow the actual authoring field at cold projection time.
/// Projected owned keys subsequently belong to their existing consumer snapshot.
/// ```compile_fail
/// use edgion_resources::resources::refs::plugin_attachment_fields::HttpPluginAttachments;
/// let fields = {
///     let owner = edgion_resources::EdgionGatewayConfig::new("config", Default::default());
///     HttpPluginAttachments::EgcGlobals(&owner)
/// };
/// fields.entries();
/// ```
#[derive(Clone, Copy)]
pub enum HttpPluginAttachments<'a> {
    Gateway(&'a Gateway),
    EgcGlobals(&'a EdgionGatewayConfig),
}
impl HttpPluginAttachments<'_> {
    pub fn entries(self) -> Vec<Result<PluginAttachmentKey, ReferenceError>> {
        match self {
            Self::Gateway(owner) => {
                let namespace = owner.metadata.namespace.as_deref().unwrap_or("default");
                let keys = owner
                    .metadata
                    .annotations
                    .as_ref()
                    .and_then(|annotations| annotations.get(EDGION_PLUGINS))
                    .map(|value| parse_plugin_ref_list(value, namespace))
                    .unwrap_or_default();
                if keys.is_empty() {
                    return Vec::new();
                }
                let identity = ObjectIdentity::checked(
                    "gateway.networking.k8s.io",
                    "Gateway",
                    ResourceScope::Namespaced(namespace),
                    owner.metadata.name.as_deref().unwrap_or_default(),
                );
                keys.into_iter()
                    .map(|key| {
                        identity?;
                        let (namespace, name) = key.split_once('/').ok_or(ReferenceError::MissingNamespace)?;
                        PluginAttachmentKey::checked(namespace, name)
                    })
                    .collect()
            }
            Self::EgcGlobals(owner) => {
                let references = owner.spec.global_plugins_ref.as_deref().unwrap_or_default();
                if references.is_empty() {
                    return Vec::new();
                }
                let identity = if owner.metadata.namespace.is_some() {
                    Err(ReferenceError::InvalidNamespace)
                } else {
                    ObjectIdentity::checked(
                        "edgion.io",
                        "EdgionGatewayConfig",
                        ResourceScope::Cluster,
                        owner.metadata.name.as_deref().unwrap_or_default(),
                    )
                };
                references
                    .iter()
                    .map(|reference| {
                        identity?;
                        PluginAttachmentKey::checked(
                            reference.namespace.as_deref().unwrap_or("default"),
                            &reference.name,
                        )
                    })
                    .collect()
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/plugin_attachment_fields_test.rs"]
mod tests;
