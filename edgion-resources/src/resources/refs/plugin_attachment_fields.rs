//! Actual plugin attachment fields; permission and runtime failure stay with consumers.
use super::{ObjectIdentity, ReferenceError, ResourceScope};
use crate::constants::annotations::edgion::{parse_plugin_ref_list, EDGION_PLUGINS, EDGION_STREAM_PLUGINS};
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

/// Stream target grammar is lexical; only the actual owner is an ObjectIdentity.
/// ```compile_fail
/// use edgion_resources::resources::refs::plugin_attachment_fields::StreamPluginAttachment;
/// let field = {
///     let owner: edgion_resources::TCPRoute = serde_json::from_str(
///         r#"{"metadata":{"name":"route"},"spec":{}}"#).unwrap();
///     StreamPluginAttachment::Tcp(&owner)
/// };
/// field.key();
/// ```
#[derive(Clone, Copy)]
pub enum StreamPluginAttachment<'a> {
    Gateway(&'a Gateway),
    Tcp(&'a crate::TCPRoute),
    Tls(&'a crate::TLSRoute),
    Udp(&'a crate::UDPRoute),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StreamAttachmentError {
    #[error("{0}")]
    InvalidOwner(ReferenceError),
    #[error("Gateway edgion-stream-plugins reference must be an unqualified resource name")]
    QualifiedGatewayName,
}

impl StreamPluginAttachment<'_> {
    /// Project once at the consumer's cold boundary. Preserve each field's existing
    /// target grammar instead of treating its lexical key as a typed target identity.
    pub fn key(self) -> Result<Option<String>, StreamAttachmentError> {
        let (metadata, kind, gateway) = match self {
            Self::Gateway(owner) => (&owner.metadata, "Gateway", true),
            Self::Tcp(owner) => (&owner.metadata, "TCPRoute", false),
            Self::Tls(owner) => (&owner.metadata, "TLSRoute", false),
            Self::Udp(owner) => (&owner.metadata, "UDPRoute", false),
        };
        let Some(value) = metadata
            .annotations
            .as_ref()
            .and_then(|annotations| annotations.get(EDGION_STREAM_PLUGINS))
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        else {
            return Ok(None);
        };
        let namespace = metadata.namespace.as_deref().unwrap_or("default");
        ObjectIdentity::checked(
            "gateway.networking.k8s.io",
            kind,
            ResourceScope::Namespaced(namespace),
            metadata.name.as_deref().unwrap_or_default(),
        )
        .map_err(StreamAttachmentError::InvalidOwner)?;
        if value.contains('/') {
            if gateway {
                Err(StreamAttachmentError::QualifiedGatewayName)
            } else {
                Ok(Some(value.to_owned()))
            }
        } else {
            Ok(Some(format!("{namespace}/{value}")))
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/plugin_attachment_fields_test.rs"]
mod tests;
