//! Shared IP / CIDR validation and IpRadixMatcher building helpers.
//!
//! Extracted from `src/types/resources/edgion_plugins/plugin_configs/ip_restriction.rs`
//! so both HTTP and L4 IP-restriction paths can share validation logic.

use std::net::IpAddr;

use crate::core::common::matcher::ip_radix_tree::IpRadixMatcher;

/// Validate IP address or CIDR range format.
pub fn validate_ip_or_cidr(ip_str: &str) -> Result<(), String> {
    if ip_str.contains('/') {
        let parts: Vec<&str> = ip_str.split('/').collect();
        if parts.len() != 2 {
            return Err(format!("Invalid CIDR format: {}", ip_str));
        }
        if parts[0].parse::<IpAddr>().is_err() {
            return Err(format!("Invalid IP address in CIDR: {}", parts[0]));
        }
        let prefix_len: u8 = parts[1]
            .parse()
            .map_err(|_| format!("Invalid prefix length in CIDR: {}", parts[1]))?;
        let ip: IpAddr = parts[0].parse().unwrap();
        let max_prefix = if ip.is_ipv4() { 32 } else { 128 };
        if prefix_len > max_prefix {
            return Err(format!(
                "Prefix length {} exceeds maximum {} for {:?}",
                prefix_len, max_prefix, ip
            ));
        }
    } else if ip_str.parse::<IpAddr>().is_err() {
        return Err(format!("Invalid IP address: {}", ip_str));
    }
    Ok(())
}

/// Convert bare IP to CIDR form (`/32` for IPv4, `/128` for IPv6). Pass-through if already CIDR.
pub fn to_cidr_format(ip_str: &str) -> Result<String, String> {
    if ip_str.contains('/') {
        Ok(ip_str.to_string())
    } else {
        let ip: IpAddr = ip_str
            .parse()
            .map_err(|_| format!("Invalid IP address: {}", ip_str))?;
        let prefix_len = if ip.is_ipv4() { 32 } else { 128 };
        Ok(format!("{}/{}", ip_str, prefix_len))
    }
}

/// Build an `IpRadixMatcher` from a list of IP / CIDR strings.
/// Validates each entry and inserts in order. Returns error on first invalid entry or build failure.
pub fn build_radix_matcher(cidrs: &[String]) -> Result<IpRadixMatcher, String> {
    let mut builder = IpRadixMatcher::builder();
    for ip_str in cidrs {
        validate_ip_or_cidr(ip_str)?;
        let cidr_str = to_cidr_format(ip_str)?;
        builder
            .insert(&cidr_str, true)
            .map_err(|e| format!("Invalid IP/CIDR '{}': {}", ip_str, e))?;
    }
    builder
        .build()
        .map_err(|e| format!("Failed to build IpRadixMatcher: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_ipv4_single_ok() {
        assert!(validate_ip_or_cidr("192.168.1.1").is_ok());
    }

    #[test]
    fn validate_ipv4_cidr_ok() {
        assert!(validate_ip_or_cidr("10.0.0.0/8").is_ok());
    }

    #[test]
    fn validate_ipv6_ok() {
        assert!(validate_ip_or_cidr("::1").is_ok());
        assert!(validate_ip_or_cidr("2001:db8::/32").is_ok());
    }

    #[test]
    fn validate_bad_ip_errs() {
        assert!(validate_ip_or_cidr("192.168.1.256").is_err());
    }

    #[test]
    fn validate_bad_cidr_prefix_errs() {
        assert!(validate_ip_or_cidr("192.168.1.0/33").is_err());
    }

    #[test]
    fn to_cidr_single_ipv4_adds_slash_32() {
        assert_eq!(to_cidr_format("10.0.0.1").unwrap(), "10.0.0.1/32");
    }

    #[test]
    fn to_cidr_single_ipv6_adds_slash_128() {
        assert_eq!(to_cidr_format("::1").unwrap(), "::1/128");
    }

    #[test]
    fn to_cidr_passthrough() {
        assert_eq!(to_cidr_format("10.0.0.0/8").unwrap(), "10.0.0.0/8");
    }

    #[test]
    fn build_matcher_hits_and_misses() {
        let m = build_radix_matcher(&[
            "192.168.1.0/24".to_string(),
            "10.0.0.1".to_string(),
        ])
        .unwrap();
        assert_eq!(m.match_ip(&"192.168.1.50".parse().unwrap()), Some(true));
        assert_eq!(m.match_ip(&"10.0.0.1".parse().unwrap()), Some(true));
        assert_eq!(m.match_ip(&"8.8.8.8".parse().unwrap()), None);
    }

    #[test]
    fn build_matcher_bad_cidr_errs() {
        assert!(build_radix_matcher(&["bogus".to_string()]).is_err());
    }
}
