use std::collections::HashMap;
use k8s_openapi::api::core::v1::{Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use crate::core::conf_sync::config_center::GatewayClassKey;
use crate::core::conf_sync::HubCache;
use crate::types::{EdgionTls, Gateway, GatewayClass, GatewayClassSpec, HTTPRoute};

pub struct ConfigHub {
    gateway_classes: HubCache<GatewayClass>,
    gateway_class_specs: HubCache<GatewayClassSpec>,
    gateways: HubCache<Gateway>,
    routes: HubCache<HTTPRoute>,
    services: HubCache<Service>,
    endpoint_slices: HubCache<EndpointSlice>,
    edgion_tls: HubCache<EdgionTls>,
    secrets: HubCache<Secret>,
}

impl ConfigHub {
}