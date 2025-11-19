/// Resource kind enumeration for different Kubernetes resource types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    GatewayClass,
    EdgionGatewayConfig,
    Gateway,
    HTTPRoute,
    Service,
    EndpointSlice,
    EdgionTls,
    Secret,
}

impl ResourceKind {
    /// Extract resource kind from content (supports both YAML and JSON formats)
    pub fn from_content(content: &str) -> Option<Self> {
        // Try JSON format first: "kind":"GatewayClass" or "kind": "GatewayClass"
        let re_json = regex::Regex::new(r#""kind"\s*:\s*"(\w+)""#).ok()?;
        if let Some(caps) = re_json.captures(content) {
            let kind_str = caps.get(1)?.as_str();
            return Self::from_kind_str(kind_str);
        }
        
        // Fallback to YAML format: "kind: GatewayClass" or "\nkind: Gateway"
        let re_yaml = regex::Regex::new(r"(?:^|\n)kind:\s*(\w+)").ok()?;
        if let Some(caps) = re_yaml.captures(content) {
            let kind_str = caps.get(1)?.as_str();
            return Self::from_kind_str(kind_str);
        }
        
        None
    }
    
    fn from_kind_str(kind_str: &str) -> Option<Self> {
        match kind_str {
            "GatewayClass" => Some(ResourceKind::GatewayClass),
            "EdgionGatewayConfig" => Some(ResourceKind::EdgionGatewayConfig),
            "Gateway" => Some(ResourceKind::Gateway),
            "HTTPRoute" => Some(ResourceKind::HTTPRoute),
            "Service" => Some(ResourceKind::Service),
            "EndpointSlice" => Some(ResourceKind::EndpointSlice),
            "EdgionTls" => Some(ResourceKind::EdgionTls),
            "Secret" => Some(ResourceKind::Secret),
            _ => None,
        }
    }

    /// Convert a kind string (e.g., "GatewayClass") to ResourceKind
    pub fn from_str_name(kind_str: &str) -> Option<Self> {
        Self::from_kind_str(kind_str)
    }

    /// Convert ResourceKind to string for match statement
    fn as_str(&self) -> &'static str {
        match self {
            ResourceKind::GatewayClass => "GatewayClass",
            ResourceKind::EdgionGatewayConfig => "GatewayClassSpec",
            ResourceKind::Gateway => "Gateway",
            ResourceKind::HTTPRoute => "HTTPRoute",
            ResourceKind::Service => "Service",
            ResourceKind::EndpointSlice => "EndpointSlice",
            ResourceKind::EdgionTls => "EdgionTls",
            ResourceKind::Secret => "Secret",
        }
    }
}
