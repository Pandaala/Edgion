//! Backend status projection without re-deserializing processed resource specs.

use std::any::Any;

use edgion_resources::{
    BackendTLSPolicy, EdgionAcme, EdgionBackend, EdgionBackendTrafficPolicy, EdgionConfigData, EdgionGatewayConfig,
    EdgionPlugins, EdgionStreamPlugins, EdgionTls, GRPCRoute, Gateway, GatewayClass, HTTPRoute, LinkSys, TCPRoute,
    TLSRoute, UDPRoute,
};
use serde_json::Value;

use super::status_observation::ObserverIdentity;
use crate::conf_mgr::ControllerRuntime;

pub(super) fn is_partitioned(kind: &str) -> bool {
    super::status_observation::is_partitioned_kind(kind)
}

pub(super) fn has_non_native_status<T: 'static>(obj: &T) -> bool {
    let obj = obj as &dyn Any;
    macro_rules! check {
        ($($ty:ty),+ $(,)?) => {
            $(if let Some(resource) = obj.downcast_ref::<$ty>() {
                return resource.status.as_ref().is_some_and(|status| status.as_local().is_none());
            })+
        };
    }
    check!(EdgionPlugins, EdgionStreamPlugins, EdgionBackend, LinkSys);
    false
}

pub(super) fn observer_identity(runtime: &ControllerRuntime) -> Option<ObserverIdentity> {
    let identity = runtime.status_identity()?;
    Some(ObserverIdentity {
        controller_name: identity.controller_name,
    })
}

/// Only replace status: compiled regexes, resolved fields and unknown-field sidecars survive.
pub(super) fn replace_status<T: 'static>(obj: &mut T, status: Value) -> Result<(), serde_json::Error> {
    let obj = obj as &mut dyn Any;
    macro_rules! replace {
        ($($ty:ty),+ $(,)?) => {
            $(if let Some(resource) = obj.downcast_mut::<$ty>() {
                resource.status = serde_json::from_value(status)?;
                return Ok(());
            })+
        };
    }
    replace!(
        EdgionPlugins,
        EdgionStreamPlugins,
        EdgionBackend,
        LinkSys,
        HTTPRoute,
        GRPCRoute,
        TCPRoute,
        TLSRoute,
        UDPRoute,
        EdgionTls,
        BackendTLSPolicy,
        EdgionBackendTrafficPolicy,
        Gateway,
        GatewayClass,
        EdgionGatewayConfig,
        EdgionConfigData,
        EdgionAcme
    );
    Ok(())
}

/// Incoming transport projections are never evidence of local ownership.
pub(super) fn clear_current_status<T: 'static>(obj: &mut T) {
    let obj = obj as &mut dyn Any;
    macro_rules! clear {
        ($($ty:ty),+ $(,)?) => {
            $(if let Some(resource) = obj.downcast_mut::<$ty>() {
                resource.spec.current_status = None;
                return;
            })+
        };
    }
    clear!(
        HTTPRoute,
        GRPCRoute,
        TCPRoute,
        TLSRoute,
        UDPRoute,
        Gateway,
        GatewayClass,
        EdgionTls,
        BackendTLSPolicy,
        EdgionBackendTrafficPolicy,
        EdgionPlugins,
        EdgionStreamPlugins,
        EdgionBackend,
        LinkSys,
        EdgionGatewayConfig,
        EdgionConfigData,
        EdgionAcme
    );
}

/// Snapshot the working native status without round-tripping a compiled spec.
/// This runs before transport finalization so Gateway proofs use current status.
pub(super) fn capture_current_status<T: 'static>(obj: &mut T) {
    let obj = obj as &mut dyn Any;
    macro_rules! native {
        ($($ty:ty),+ $(,)?) => {
            $(if let Some(resource) = obj.downcast_mut::<$ty>() {
                resource.spec.current_status = resource.status.clone();
                return;
            })+
        };
    }
    native!(
        HTTPRoute,
        GRPCRoute,
        TCPRoute,
        TLSRoute,
        UDPRoute,
        Gateway,
        GatewayClass,
        EdgionTls,
        BackendTLSPolicy,
        EdgionBackendTrafficPolicy,
        EdgionGatewayConfig,
        EdgionConfigData,
        EdgionAcme
    );
    macro_rules! partitioned {
        ($($ty:ty),+ $(,)?) => {
            $(if let Some(resource) = obj.downcast_mut::<$ty>() {
                resource.spec.current_status = resource.status.as_ref().and_then(|status| status.as_local()).cloned();
                return;
            })+
        };
    }
    partitioned!(EdgionPlugins, EdgionStreamPlugins, EdgionBackend, LinkSys);
}

/// Even native FS/etcd status may retain foreign parents for diagnostics. Only
/// the identity overwritten by finalize_transport can select serving results.
pub(super) fn filter_current_route_status<T: 'static>(obj: &mut T) {
    let obj = obj as &mut dyn Any;
    macro_rules! filter {
        ($($ty:ty),+ $(,)?) => {
            $(if let Some(resource) = obj.downcast_mut::<$ty>() {
                if let Some(status) = resource.spec.current_status.as_mut() {
                    let owner = resource.spec.resolved_status_controller.as_deref().filter(|owner| !owner.is_empty());
                    status.parents.retain(|parent| Some(parent.controller_name.as_str()) == owner);
                }
                return;
            })+
        };
    }
    filter!(HTTPRoute, GRPCRoute, TCPRoute, TLSRoute, UDPRoute);
}

/// Native attachment handlers supply their configured owner during finalization.
/// API history stays complete while current status excludes foreign observations.
pub(super) fn filter_current_native_attachment_status<T: 'static>(obj: &mut T, configured_owner: &str) {
    let obj = obj as &mut dyn Any;
    let configured_owner = Some(configured_owner);
    let owner = configured_owner.filter(|owner| !owner.is_empty());
    if let Some(resource) = obj.downcast_mut::<EdgionTls>() {
        if let Some(status) = resource.spec.current_status.as_mut() {
            status
                .parents
                .retain(|parent| Some(parent.controller_name.as_str()) == owner);
        }
        return;
    }
    macro_rules! policies {
        ($($ty:ty),+ $(,)?) => {
            $(if let Some(resource) = obj.downcast_mut::<$ty>() {
                if let Some(status) = resource.spec.current_status.as_mut() {
                    status.ancestors.retain(|ancestor| Some(ancestor.controller_name.as_str()) == owner);
                }
                return;
            })+
        };
    }
    policies!(BackendTLSPolicy, EdgionBackendTrafficPolicy);
}

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/status_pipeline_test.rs"]
mod tests;
