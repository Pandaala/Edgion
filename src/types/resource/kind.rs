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
    EdgionStreamPlugins = 16,
    ReferenceGrant = 17,
    BackendTLSPolicy = 18,
    Endpoint = 19,
}

impl ResourceKind {
    /// Get the static string representation of this ResourceKind (PascalCase)
    ///
    /// This is preferred over `format!("{:?}", kind)` because:
    /// - Returns `&'static str` instead of `String`
    /// - No heap allocation
    /// - Consistent and stable (doesn't depend on Debug trait implementation)
    pub const fn as_str(&self) -> &'static str {
        match self {
            ResourceKind::Unspecified => "Unspecified",
            ResourceKind::GatewayClass => "GatewayClass",
            ResourceKind::EdgionGatewayConfig => "EdgionGatewayConfig",
            ResourceKind::Gateway => "Gateway",
            ResourceKind::HTTPRoute => "HTTPRoute",
            ResourceKind::Service => "Service",
            ResourceKind::EndpointSlice => "EndpointSlice",
            ResourceKind::EdgionTls => "EdgionTls",
            ResourceKind::Secret => "Secret",
            ResourceKind::EdgionPlugins => "EdgionPlugins",
            ResourceKind::GRPCRoute => "GRPCRoute",
            ResourceKind::TCPRoute => "TCPRoute",
            ResourceKind::UDPRoute => "UDPRoute",
            ResourceKind::PluginMetaData => "PluginMetaData",
            ResourceKind::TLSRoute => "TLSRoute",
            ResourceKind::LinkSys => "LinkSys",
            ResourceKind::EdgionStreamPlugins => "EdgionStreamPlugins",
            ResourceKind::ReferenceGrant => "ReferenceGrant",
            ResourceKind::BackendTLSPolicy => "BackendTLSPolicy",
            ResourceKind::Endpoint => "Endpoint",
        }
    }

    /// Compile-time exhaustiveness check
    ///
    /// This function ensures that all ResourceKind variants are defined in resource_defs.rs.
    /// If you add a new ResourceKind variant, the compiler will fail here until you also
    /// add the corresponding entry in resource_defs.rs.
    ///
    /// This check runs at compile time via const evaluation.
    #[allow(dead_code)]
    const fn _compile_time_sync_check() {
        // This match must cover all ResourceKind variants.
        // If a new variant is added to ResourceKind but not to resource_defs.rs,
        // the exhaustive check in resource_defs.rs will catch it.
        //
        // Conversely, if a variant exists in resource_defs.rs but not here,
        // the macro expansion will fail because ResourceKind::VariantName won't exist.
        //
        // This creates a bidirectional compile-time check.
        const fn check(kind: ResourceKind) {
            match kind {
                ResourceKind::Unspecified => {}
                ResourceKind::GatewayClass => {}
                ResourceKind::EdgionGatewayConfig => {}
                ResourceKind::Gateway => {}
                ResourceKind::HTTPRoute => {}
                ResourceKind::Service => {}
                ResourceKind::EndpointSlice => {}
                ResourceKind::EdgionTls => {}
                ResourceKind::Secret => {}
                ResourceKind::EdgionPlugins => {}
                ResourceKind::GRPCRoute => {}
                ResourceKind::TCPRoute => {}
                ResourceKind::UDPRoute => {}
                ResourceKind::PluginMetaData => {}
                ResourceKind::TLSRoute => {}
                ResourceKind::LinkSys => {}
                ResourceKind::EdgionStreamPlugins => {}
                ResourceKind::ReferenceGrant => {}
                ResourceKind::BackendTLSPolicy => {}
                ResourceKind::Endpoint => {} // NOTE: When adding a new ResourceKind variant:
                                             // 1. Add it to this match
                                             // 2. Add corresponding entry in src/types/resource_defs.rs
                                             // The compiler will ensure both are in sync.
            }
        }
        // Trigger the check
        check(ResourceKind::Unspecified);
    }

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
            "edgionstreamplugins" => Some(ResourceKind::EdgionStreamPlugins),
            "referencegrant" | "referencegrants" => Some(ResourceKind::ReferenceGrant),
            "backendtlspolicy" | "backendtlspolicies" => Some(ResourceKind::BackendTLSPolicy),
            "endpoint" | "endpoints" => Some(ResourceKind::Endpoint),
            _ => None,
        }
    }
}
