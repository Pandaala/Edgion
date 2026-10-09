//! Actual Stream plugin fields; source checking does not redefine target grammar.
use super::{ObjectIdentity, ReferenceError, ResourceScope};
use crate::resources::edgion_stream_plugins::{ConnectionRateLimitConfig, EdgionStreamPlugin};
use crate::EdgionStreamPlugins;

/// A cold loan of the actual connection limiter configuration and its owner.
/// No prepared catalogue or provider handle is retained.
///
/// ```compile_fail
/// use edgion_resources::EdgionStreamPlugins;
/// use edgion_resources::resources::refs::stream_plugin_fields::StreamConnectionRateLimitField;
/// fn cannot_drop_owner(owner: EdgionStreamPlugins) {
///     let field = StreamConnectionRateLimitField::from_plugins(&owner, 0).unwrap();
///     let config = field.configuration().unwrap();
///     drop(owner);
///     let _ = config.needs_redis();
/// }
/// ```
/// The actual owner cannot mutate while the field's configuration is borrowed.
///
/// ```compile_fail
/// use edgion_resources::EdgionStreamPlugins;
/// use edgion_resources::resources::refs::stream_plugin_fields::StreamConnectionRateLimitField;
/// fn cannot_change_owner(mut owner: EdgionStreamPlugins) {
///     let config = StreamConnectionRateLimitField::from_plugins(&owner, 0)
///         .unwrap().configuration().unwrap();
///     owner.metadata.name = None;
///     let _ = config.needs_redis();
/// }
/// ```
pub struct StreamConnectionRateLimitField<'a> {
    owner: &'a EdgionStreamPlugins,
    config: &'a ConnectionRateLimitConfig,
    enabled: bool,
}

impl<'a> StreamConnectionRateLimitField<'a> {
    pub fn from_plugins(owner: &'a EdgionStreamPlugins, index: usize) -> Option<Self> {
        let entry = owner.spec.plugins.as_ref()?.get(index)?;
        let EdgionStreamPlugin::ConnectionRateLimit(config) = &entry.plugin else {
            return None;
        };
        Some(Self {
            owner,
            config,
            enabled: entry.is_enabled(),
        })
    }

    /// Check only an active external field's source; schema validation owns the target.
    pub fn configuration(self) -> Result<&'a ConnectionRateLimitConfig, ReferenceError> {
        if self.enabled && self.config.needs_redis() {
            ObjectIdentity::checked(
                "edgion.io",
                "EdgionStreamPlugins",
                ResourceScope::Namespaced(self.owner.metadata.namespace.as_deref().unwrap_or("default")),
                self.owner.metadata.name.as_deref().unwrap_or_default(),
            )?;
        }
        Ok(self.config)
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/stream_plugin_fields_test.rs"]
mod tests;
