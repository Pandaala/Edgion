//! Types for IP radix matching

use super::error::IpRadixError;
use std::net::IpAddr;
use std::str::FromStr;

/// Represents a CIDR (Classless Inter-Domain Routing) notation
///
/// Supports both IPv4 and IPv6 CIDR notations:
/// - IPv4: "192.168.1.0/24", "10.0.0.0/8"
/// - IPv6: "2001:db8::/32", "fe80::/10"
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpCidr {
    /// IPv4 CIDR with 32-bit address and prefix length (0-32)
    V4 {
        /// The IPv4 address as u32 (network byte order)
        addr: u32,
        /// The prefix length (0-32)
        prefix_len: u8,
    },
    /// IPv6 CIDR with 128-bit address and prefix length (0-128)
    V6 {
        /// The IPv6 address as u128 (network byte order)
        addr: u128,
        /// The prefix length (0-128)
        prefix_len: u8,
    },
}

impl IpCidr {
    /// Parse a CIDR string into IpCidr
    ///
    /// # Examples
    /// ```
    /// use edgion_algo::matcher::radix_ip::IpCidr;
    ///
    /// let cidr_v4 = IpCidr::parse("192.168.1.0/24").unwrap();
    /// let cidr_v6 = IpCidr::parse("2001:db8::/32").unwrap();
    /// ```
    pub fn parse(cidr_str: &str) -> Result<Self, IpRadixError> {
        let parts: Vec<&str> = cidr_str.split('/').collect();

        if parts.len() != 2 {
            return Err(IpRadixError::InvalidCidr {
                input: cidr_str.to_string(),
                reason: "CIDR must be in format 'address/prefix_len'".to_string(),
            });
        }

        let ip_str = parts[0];
        let prefix_str = parts[1];

        let prefix_len = prefix_str.parse::<u8>().map_err(|e| IpRadixError::InvalidCidr {
            input: cidr_str.to_string(),
            reason: format!("invalid prefix length: {}", e),
        })?;

        let ip = IpAddr::from_str(ip_str).map_err(|e| IpRadixError::InvalidIpAddress {
            input: ip_str.to_string(),
            error: e.to_string(),
        })?;

        match ip {
            IpAddr::V4(ipv4) => {
                if prefix_len > 32 {
                    return Err(IpRadixError::PrefixTooLong { prefix_len, max: 32 });
                }

                let addr = u32::from(ipv4);

                // Normalize: zero out bits beyond prefix length
                let normalized_addr = if prefix_len == 0 {
                    0
                } else if prefix_len >= 32 {
                    addr
                } else {
                    let mask = !0u32 << (32 - prefix_len);
                    addr & mask
                };

                Ok(IpCidr::V4 {
                    addr: normalized_addr,
                    prefix_len,
                })
            }
            IpAddr::V6(ipv6) => {
                if prefix_len > 128 {
                    return Err(IpRadixError::PrefixTooLong { prefix_len, max: 128 });
                }

                let addr = u128::from(ipv6);

                // Normalize: zero out bits beyond prefix length
                let normalized_addr = if prefix_len == 0 {
                    0
                } else if prefix_len >= 128 {
                    addr
                } else {
                    let mask = !0u128 << (128 - prefix_len);
                    addr & mask
                };

                Ok(IpCidr::V6 {
                    addr: normalized_addr,
                    prefix_len,
                })
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/matcher/radix_ip/types_test.rs"]
mod tests;
