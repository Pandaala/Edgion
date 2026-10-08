//! Cluster-scoped parameters selected by an actual GatewayClass field.
use super::{ObjectIdentity, ReferenceError, ResourceScope};
use crate::GatewayClass;

/// A validated parameters target borrowing its actual owning class.
/// This lookup identity conveys no ReferenceGrant or material authority.
/// ```compile_fail
/// use edgion_resources::resources::refs::gateway_class_fields::GatewayClassParametersRef;
/// fn release_owner(class: edgion_resources::GatewayClass) {
///     let reference = GatewayClassParametersRef::from_class(&class).unwrap().unwrap();
///     drop(class);
///     reference.target();
/// }
/// ```
pub struct GatewayClassParametersRef<'a> {
    _owner: &'a GatewayClass,
    target: ObjectIdentity<'a>,
}

impl<'a> GatewayClassParametersRef<'a> {
    pub fn from_class(class: &'a GatewayClass) -> Result<Option<Self>, ReferenceError> {
        let Some(field) = class.spec.parameters_ref.as_ref() else {
            return Ok(None);
        };
        if field.group != "edgion.io" || field.kind != "EdgionGatewayConfig" {
            return Err(ReferenceError::UnsupportedTarget);
        }
        if field.namespace.is_some() {
            return Err(ReferenceError::InvalidNamespace);
        }
        let target = ObjectIdentity::checked(&field.group, &field.kind, ResourceScope::Cluster, &field.name)?;
        Ok(Some(Self { _owner: class, target }))
    }

    pub fn target(&self) -> ObjectIdentity<'a> {
        self.target
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/gateway_class_fields_test.rs"]
mod tests;
