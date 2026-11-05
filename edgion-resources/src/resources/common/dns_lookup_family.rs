//! Address families requested for backend DNS discovery.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum DnsLookupFamily {
    #[default]
    Ipv4Only,
    Ipv6Only,
    All,
}

impl DnsLookupFamily {
    /// IPv4-mapped IPv6 addresses belong to the IPv4 family everywhere.
    pub fn allows(self, address: std::net::IpAddr) -> bool {
        match self {
            Self::Ipv4Only => address.to_canonical().is_ipv4(),
            Self::Ipv6Only => address.to_canonical().is_ipv6(),
            Self::All => true,
        }
    }
    /// Absence inherits; every present value must use the exact public spelling.
    pub fn from_annotation(value: Option<&str>) -> Result<Option<Self>, &'static str> {
        value
            .map(|value| match value {
                "ipv4Only" => Ok(Self::Ipv4Only),
                "ipv6Only" => Ok(Self::Ipv6Only),
                "all" => Ok(Self::All),
                _ => Err("dns-lookup-family must be ipv4Only, ipv6Only, or all"),
            })
            .transpose()
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/common/dns_lookup_family_test.rs"]
mod tests;
