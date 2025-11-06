/// Resource kind enumeration for different Kubernetes resource types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    GatewayClass,
    GatewayClassSpec,
    Gateway,
    HTTPRoute,
    Service,
    EndpointSlice,
    EdgionTls,
    Secret,
}

impl ResourceKind {
    /// Extract resource kind from content by matching "\nkind: *\n" pattern
    pub fn from_content(content: &str) -> Option<Self> {
        // Match pattern like "\nkind: GatewayClass\n" or "\nkind: Gateway\n"
        let re = regex::Regex::new(r"\nkind:\s*(\w+)\s*\n").ok()?;
        let caps = re.captures(content)?;
        let kind_str = caps.get(1)?.as_str();

        match kind_str {
            "GatewayClass" => Some(ResourceKind::GatewayClass),
            "GatewayClassSpec" => Some(ResourceKind::GatewayClassSpec),
            "Gateway" => Some(ResourceKind::Gateway),
            "HTTPRoute" => Some(ResourceKind::HTTPRoute),
            "Service" => Some(ResourceKind::Service),
            "EndpointSlice" => Some(ResourceKind::EndpointSlice),
            "EdgionTls" => Some(ResourceKind::EdgionTls),
            "Secret" => Some(ResourceKind::Secret),
            _ => None,
        }
    }

    /// Convert ResourceKind to string for match statement
    fn as_str(&self) -> &'static str {
        match self {
            ResourceKind::GatewayClass => "GatewayClass",
            ResourceKind::GatewayClassSpec => "GatewayClassSpec",
            ResourceKind::Gateway => "Gateway",
            ResourceKind::HTTPRoute => "HTTPRoute",
            ResourceKind::Service => "Service",
            ResourceKind::EndpointSlice => "EndpointSlice",
            ResourceKind::EdgionTls => "EdgionTls",
            ResourceKind::Secret => "Secret",
        }
    }
}

