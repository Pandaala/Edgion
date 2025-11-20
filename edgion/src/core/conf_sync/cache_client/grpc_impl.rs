use crate::core::conf_sync::cache_client::GrpcSyncable;
use crate::types::{EdgionTls, HTTPRoute, ResourceKind};
use k8s_openapi::api::core::v1::{Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;

// Implement GrpcSyncable for HTTPRoute
impl GrpcSyncable for HTTPRoute {
    fn resource_kind() -> ResourceKind {
        ResourceKind::HTTPRoute
    }

    fn kind_name() -> &'static str {
        "HTTPRoute"
    }
}

// Implement GrpcSyncable for Service
impl GrpcSyncable for Service {
    fn resource_kind() -> ResourceKind {
        ResourceKind::Service
    }

    fn kind_name() -> &'static str {
        "Service"
    }
}

// Implement GrpcSyncable for EndpointSlice
impl GrpcSyncable for EndpointSlice {
    fn resource_kind() -> ResourceKind {
        ResourceKind::EndpointSlice
    }

    fn kind_name() -> &'static str {
        "EndpointSlice"
    }
}

// Implement GrpcSyncable for EdgionTls
impl GrpcSyncable for EdgionTls {
    fn resource_kind() -> ResourceKind {
        ResourceKind::EdgionTls
    }

    fn kind_name() -> &'static str {
        "EdgionTls"
    }
}

// Implement GrpcSyncable for Secret
impl GrpcSyncable for Secret {
    fn resource_kind() -> ResourceKind {
        ResourceKind::Secret
    }

    fn kind_name() -> &'static str {
        "Secret"
    }
}
