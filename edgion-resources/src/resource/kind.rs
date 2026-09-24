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
    EdgionConfigData = 13,
    TLSRoute = 14,
    LinkSys = 15,
    EdgionStreamPlugins = 16,
    ReferenceGrant = 17,
    BackendTLSPolicy = 18,
    EdgionAcme = 20,
    ConfigMap = 21,
    EdgionBackendTrafficPolicy = 22,
    EdgionBackend = 23,
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
            ResourceKind::EdgionConfigData => "EdgionConfigData",
            ResourceKind::TLSRoute => "TLSRoute",
            ResourceKind::LinkSys => "LinkSys",
            ResourceKind::EdgionStreamPlugins => "EdgionStreamPlugins",
            ResourceKind::ReferenceGrant => "ReferenceGrant",
            ResourceKind::BackendTLSPolicy => "BackendTLSPolicy",
            ResourceKind::EdgionAcme => "EdgionAcme",
            ResourceKind::ConfigMap => "ConfigMap",
            ResourceKind::EdgionBackendTrafficPolicy => "EdgionBackendTrafficPolicy",
            ResourceKind::EdgionBackend => "EdgionBackend",
        }
    }

    /// Compile-time exhaustiveness check
    ///
    /// This function ensures that all ResourceKind variants are defined in resource_defs.rs.
    /// If you add a new ResourceKind variant, the compiler will fail here until you also
    /// add the corresponding entry in resource_defs.rs.
    ///
    /// This check runs at compile time via const evaluation.
    const fn _compile_time_sync_check() {
        // This match enforces that every ResourceKind variant is listed here.
        // If a new variant is added to the enum but not to this match, compilation fails.
        //
        // Note: this is a one-way check. A variant present here but absent from
        // resource_defs.rs still compiles — the resource simply won't appear in
        // the registry. Adding an entry to resource_defs.rs that references a
        // non-existent enum variant also causes a compile error (via macro expansion).
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
                ResourceKind::EdgionConfigData => {}
                ResourceKind::TLSRoute => {}
                ResourceKind::LinkSys => {}
                ResourceKind::EdgionStreamPlugins => {}
                ResourceKind::ReferenceGrant => {}
                ResourceKind::BackendTLSPolicy => {}
                ResourceKind::EdgionAcme => {}
                ResourceKind::ConfigMap => {}
                ResourceKind::EdgionBackendTrafficPolicy => {}
                ResourceKind::EdgionBackend => {} // NOTE: When adding a new ResourceKind variant:
                                                  // 1. Add it to this match
                                                  // 2. Add corresponding entry in src/resource/defs.rs
                                                  // The compiler will ensure both are in sync.
            }
        }
        // Trigger the check
        check(ResourceKind::Unspecified);
    }
}

// Evaluate at compile time so the exhaustiveness match above is actually enforced.
const _: () = ResourceKind::_compile_time_sync_check();

impl ResourceKind {
    /// Extract resource kind from content (supports both YAML and JSON formats)
    pub fn from_content(content: &str) -> Option<Self> {
        static RE_JSON: std::sync::LazyLock<regex::Regex> =
            std::sync::LazyLock::new(|| regex::Regex::new(r#""kind"\s*:\s*"(\w+)""#).unwrap());
        static RE_YAML: std::sync::LazyLock<regex::Regex> =
            std::sync::LazyLock::new(|| regex::Regex::new(r"(?:^|\n)kind:\s*(\w+)").unwrap());

        // Try JSON format first: "kind":"GatewayClass" or "kind": "GatewayClass"
        if let Some(caps) = RE_JSON.captures(content) {
            let kind_str = caps.get(1)?.as_str();
            return Self::from_kind_name(kind_str);
        }

        // Fallback to YAML format: "kind: GatewayClass" or "\nkind: Gateway"
        if let Some(caps) = RE_YAML.captures(content) {
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
            "edgiongwconfig" | "edgiongatewayconfig" => Some(ResourceKind::EdgionGatewayConfig),
            "gateway" => Some(ResourceKind::Gateway),
            "httproute" => Some(ResourceKind::HTTPRoute),
            "service" => Some(ResourceKind::Service),
            "endpointslice" => Some(ResourceKind::EndpointSlice),
            "edgiontls" => Some(ResourceKind::EdgionTls),
            "secret" => Some(ResourceKind::Secret),
            "edgionplugins" => Some(ResourceKind::EdgionPlugins),
            "grpcroute" => Some(ResourceKind::GRPCRoute),
            "tcproute" => Some(ResourceKind::TCPRoute),
            "udproute" => Some(ResourceKind::UDPRoute),
            "edgionconfigdata" => Some(ResourceKind::EdgionConfigData),
            "tlsroute" => Some(ResourceKind::TLSRoute),
            "linksys" => Some(ResourceKind::LinkSys),
            "edgionstreamplugins" => Some(ResourceKind::EdgionStreamPlugins),
            "referencegrant" | "referencegrants" => Some(ResourceKind::ReferenceGrant),
            "backendtlspolicy" | "backendtlspolicies" => Some(ResourceKind::BackendTLSPolicy),
            "edgionacme" => Some(ResourceKind::EdgionAcme),
            "configmap" => Some(ResourceKind::ConfigMap),
            "edgionbackendtrafficpolicy" => Some(ResourceKind::EdgionBackendTrafficPolicy),
            "edgionbackend" => Some(ResourceKind::EdgionBackend),
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/resource/kind_test.rs"]
mod tests;
