/// Resource kind enumeration for different Kubernetes resource types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, ::prost::Enumeration)]
#[repr(i32)]
pub enum ResourceKind {
    Unspecified = 0,
    GatewayClass = 1,
    EdgionGatewayConfig = 2,
    Gateway = 3,
    HTTPRoute = 4,
    Service = 5,
    EndpointSlice = 6,
    EdgionTls = 7,
    Secret = 8,
    EdgionPlugins = 9,
    GRPCRoute = 10,
    TCPRoute = 11,
    UDPRoute = 12,
    PluginMetaData = 13,
    TLSRoute = 14,
    LinkSys = 15,
}

impl ResourceKind {
    /// Extract resource kind from content (supports both YAML and JSON formats)
    pub fn from_content(content: &str) -> Option<Self> {
        // Try JSON format first: "kind":"GatewayClass" or "kind": "GatewayClass"
        let re_json = regex::Regex::new(r#""kind"\s*:\s*"(\w+)""#).ok()?;
        if let Some(caps) = re_json.captures(content) {
            let kind_str = caps.get(1)?.as_str();
            return Self::from_kind_name(kind_str);
        }

        // Fallback to YAML format: "kind: GatewayClass" or "\nkind: Gateway"
        let re_yaml = regex::Regex::new(r"(?:^|\n)kind:\s*(\w+)").ok()?;
        if let Some(caps) = re_yaml.captures(content) {
            let kind_str = caps.get(1)?.as_str();
            return Self::from_kind_name(kind_str);
        }

        None
    }

    pub fn from_kind_name(kind_str: &str) -> Option<Self> {
        // Case-insensitive matching for API convenience
        match kind_str.to_lowercase().as_str() {
            "unspecified" => Some(ResourceKind::Unspecified),
            "gatewayclass" => Some(ResourceKind::GatewayClass),
            "edgiongwconfig" | "edgiongatewayconfig" | "ztracegatewayconfig" => Some(ResourceKind::EdgionGatewayConfig),
            "gateway" => Some(ResourceKind::Gateway),
            "httproute" => Some(ResourceKind::HTTPRoute),
            "service" => Some(ResourceKind::Service),
            "endpointslice" => Some(ResourceKind::EndpointSlice),
            "edgiontls" | "ztracetls" => Some(ResourceKind::EdgionTls),
            "secret" => Some(ResourceKind::Secret),
            "edgionplugins" => Some(ResourceKind::EdgionPlugins),
            "grpcroute" => Some(ResourceKind::GRPCRoute),
            "tcproute" => Some(ResourceKind::TCPRoute),
            "udproute" => Some(ResourceKind::UDPRoute),
            "pluginmetadata" => Some(ResourceKind::PluginMetaData),
            "tlsroute" => Some(ResourceKind::TLSRoute),
            "linksys" => Some(ResourceKind::LinkSys),
            _ => None,
        }
    }
}
