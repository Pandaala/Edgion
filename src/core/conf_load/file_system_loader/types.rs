use crate::core::utils::ResourceMetadata;
use crate::types::prelude_resources::*;
use k8s_openapi::api::core::v1::{Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;

#[derive(Clone)]
pub struct FileInfo {
    pub metadata: ResourceMetadata,
    pub content: String,
}

/// Enum to hold parsed resource objects
pub enum ParsedResource {
    HTTPRoute(HTTPRoute),
    GRPCRoute(GRPCRoute),
    TCPRoute(TCPRoute),
    UDPRoute(UDPRoute),
    TLSRoute(TLSRoute),
    Service(Service),
    EndpointSlice(EndpointSlice),
    EdgionTls(EdgionTls),
    EdgionPlugins(EdgionPlugins),
    PluginMetaData(PluginMetaData),
    LinkSys(LinkSys),
    Secret(Secret),
}
