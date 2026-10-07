//! Shared Gateway API `ExtensionRef` validation.

use crate::resources::http_route::LocalObjectReference;

/// The only `ExtensionRef` target implemented by Edgion.
pub const EDGION_PLUGINS_GROUP: &str = "edgion.io";
pub const EDGION_PLUGINS_KIND: &str = "EdgionPlugins";

/// Static failures in the Gateway API filter discriminator/payload contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionRefValidationError {
    MissingConfiguration,
    UnexpectedConfiguration,
    UnsupportedTarget,
}

/// Return whether a local reference names Edgion's supported plugin resource.
pub fn is_supported_edgion_plugins_ref(reference: &LocalObjectReference) -> bool {
    reference.group == EDGION_PLUGINS_GROUP && reference.kind == EDGION_PLUGINS_KIND
}

/// Validate the Gateway API `type`/`extensionRef` union and Edgion's supported target.
///
/// An empty group denotes the Kubernetes core API group; it never aliases the
/// `edgion.io` API group.
pub fn validate_extension_ref_filter(
    is_extension_ref: bool,
    extension_ref: Option<&LocalObjectReference>,
) -> Result<Option<&LocalObjectReference>, ExtensionRefValidationError> {
    match (is_extension_ref, extension_ref) {
        (false, None) => Ok(None),
        (false, Some(_)) => Err(ExtensionRefValidationError::UnexpectedConfiguration),
        (true, None) => Err(ExtensionRefValidationError::MissingConfiguration),
        (true, Some(reference)) if is_supported_edgion_plugins_ref(reference) => Ok(Some(reference)),
        (true, Some(_)) => Err(ExtensionRefValidationError::UnsupportedTarget),
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/extension_ref_test.rs"]
mod tests;
