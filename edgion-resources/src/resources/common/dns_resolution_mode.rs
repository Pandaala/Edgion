//! How an `ExternalName` Service backend is resolved.
//!
//! One enum and one parser shared by the Controller and the Gateway, so both
//! planes always agree on which Services the Controller publishes for.

/// Resolution mode of an `ExternalName` Service backend.
///
/// The Service annotation `edgion.io/dns-resolution-mode` selects it per
/// Service; without the annotation the Gateway uses the global default from
/// `EdgionGatewayConfig.spec.dnsResolver.endpointTable` (`gateway` when true,
/// `system` when false). `controller` is only ever selected per Service.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum DnsResolutionMode {
    /// The Gateway resolves in the background and publishes the full address
    /// set as a Gateway-local EndpointSlice (ordinary LB, health check and
    /// outlier ejection apply).
    Gateway,
    /// The Gateway asks the OS resolver on every request, connection or UDP
    /// session; no Edgion cache and no endpoint table.
    System,
    /// The Controller resolves and publishes the endpoints; the Gateway reads
    /// only those.
    Controller,
}

impl DnsResolutionMode {
    pub const GATEWAY: &'static str = "gateway";
    pub const SYSTEM: &'static str = "system";
    pub const CONTROLLER: &'static str = "controller";

    /// Absence inherits the global default; every present value must use the
    /// exact public spelling. An unknown value is an error (the backend fails
    /// closed) rather than a silent fallback to another mode.
    pub fn from_annotation(value: Option<&str>) -> Result<Option<Self>, &'static str> {
        value
            .map(|value| match value {
                Self::GATEWAY => Ok(Self::Gateway),
                Self::SYSTEM => Ok(Self::System),
                Self::CONTROLLER => Ok(Self::Controller),
                _ => Err("dns-resolution-mode must be gateway, system, or controller"),
            })
            .transpose()
    }

    /// The global default selected by `EdgionGatewayConfig.spec.dnsResolver.endpointTable`.
    pub fn global_default(endpoint_table: bool) -> Self {
        if endpoint_table {
            Self::Gateway
        } else {
            Self::System
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gateway => Self::GATEWAY,
            Self::System => Self::SYSTEM,
            Self::Controller => Self::CONTROLLER,
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/common/dns_resolution_mode_test.rs"]
mod tests;
