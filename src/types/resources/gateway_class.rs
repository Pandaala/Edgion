//! GatewayClass resource definition
//!
//! GatewayClass defines a class of Gateways that can be instantiated

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use super::common::Condition;

/// API group for GatewayClass
pub const GATEWAY_CLASS_GROUP: &str = "gateway.networking.k8s.io";

/// Kind for GatewayClass
pub const GATEWAY_CLASS_KIND: &str = "GatewayClass";

/// GatewayClass defines a class of Gateways that can be instantiated
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1",
    kind = "GatewayClass",
    plural = "gatewayclasses",
    status = "GatewayClassStatus"
)]
#[serde(rename_all = "camelCase")]
pub struct GatewayClassSpec {
    /// ControllerName is the name of the controller
    pub controller_name: String,

    /// Description is a human-readable description of the class
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// ParametersRef references a resource that contains parameters
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters_ref: Option<ParametersReference>,
}

/// GatewayClassStatus describes the status of the GatewayClass resource.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct GatewayClassStatus {
    /// Conditions describe the current conditions of the GatewayClass.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ParametersReference {
    /// Group is the group of the referent
    pub group: String,

    /// Kind is the kind of the referent
    pub kind: String,

    /// Name is the name of the referent
    pub name: String,

    /// Namespace is the namespace of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}
