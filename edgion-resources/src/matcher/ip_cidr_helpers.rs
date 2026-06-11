//! Shared IP / CIDR validation and IpRadixMatcher building helpers.
//!
//! Extracted from `src/types/resources/edgion_plugins/plugin_configs/ip_restriction.rs`
//! so both HTTP and L4 IP-restriction paths can share validation logic.

use std::collections::HashSet;
use std::net::IpAddr;

use crate::matcher::ip_radix_tree::IpRadixMatcher;
use crate::resources::edgion_plugins::{AccessDecision, DefaultAction};
use crate::resources::plugin_metadata::IpGroup;

pub use crate::resources::common::group_name::validate_group_name;

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
        let ip: IpAddr = ip_str.parse().map_err(|_| format!("Invalid IP address: {}", ip_str))?;
        let prefix_len = if ip.is_ipv4() { 32 } else { 128 };
        Ok(format!("{}/{}", ip_str, prefix_len))
    }
}

/// Lightweight validation of an `IpGroup` list:
/// - name charset/length per K8s label-value rules
/// - name uniqueness within the list
/// - per-group `cidrs` non-empty
/// - each CIDR parses as IPv4/IPv6
/// - cross-group exact-duplicate CIDR detection (after normalisation)
///
/// Does NOT build a matcher. Suitable for admission-webhook hot path.
pub fn validate_groups_structure(groups: &[IpGroup]) -> Result<(), String> {
    if groups.is_empty() {
        return Err("group list is empty".to_string());
    }
    let mut seen_names: HashSet<&str> = HashSet::new();
    let mut seen_cidrs: HashSet<String> = HashSet::new();
    for g in groups {
        validate_group_name(&g.name)?;
        if !seen_names.insert(g.name.as_str()) {
            return Err(format!("duplicate group name '{}'", g.name));
        }
        if g.cidrs.is_empty() {
            return Err(format!("group '{}' has no cidrs", g.name));
        }
        for cidr in &g.cidrs {
            validate_ip_or_cidr(cidr).map_err(|e| format!("group '{}': {}", g.name, e))?;
            let norm = to_cidr_format(cidr).map_err(|e| format!("group '{}': {}", g.name, e))?;
            if !seen_cidrs.insert(norm.clone()) {
                return Err(format!("duplicate CIDR '{}' across groups", norm));
            }
        }
    }
    Ok(())
}

/// Full build: run `validate_groups_structure` first, then construct the
/// matcher. Error messages are identical between the two paths for the
/// same invalid input.
pub fn build_grouped_matcher(groups: &[IpGroup]) -> Result<IpRadixMatcher, String> {
    validate_groups_structure(groups)?;
    let mut builder = IpRadixMatcher::builder();
    for g in groups {
        for cidr in &g.cidrs {
            let norm = to_cidr_format(cidr).map_err(|e| format!("group '{}': {}", g.name, e))?;
            builder
                .insert(&norm, &g.name)
                .map_err(|e| format!("group '{}': {}", g.name, e))?;
        }
    }
    builder
        .build()
        .map_err(|e| format!("Failed to build IpRadixMatcher: {}", e))
}

/// Evaluate an IP against `deny`/`allow` matchers with the canonical
/// deny > allow > default priority used by every IP-restriction plugin
/// (HTTP `IpRestrictionConfig`, L4 `ConnectionIpRestrictionRules`, and
/// the cluster-scope `GlobalConnectionIpRestriction` profile).
///
/// Behaviour:
/// - `deny` match wins → `DenyByGroup(g)`.
/// - `allow` is present and matches → `AllowByGroup(g)`.
/// - `allow` is present but does not match → `DenyByWhitelistMiss`.
/// - `allow` is absent → fall through to `Default(default_action.clone())`.
///
/// Keeping the algorithm in one place prevents the three call sites from
/// silently diverging when the priority or returned variants change. See
/// spec §4.6 truth table.
pub fn evaluate_ip_access(
    deny: Option<&IpRadixMatcher>,
    allow: Option<&IpRadixMatcher>,
    default_action: &DefaultAction,
    ip: &IpAddr,
) -> AccessDecision {
    if let Some(d) = deny {
        if let Some(g) = d.matched_group_arc(ip) {
            return AccessDecision::DenyByGroup(g);
        }
    }
    if let Some(a) = allow {
        if let Some(g) = a.matched_group_arc(ip) {
            return AccessDecision::AllowByGroup(g);
        }
        return AccessDecision::DenyByWhitelistMiss;
    }
    AccessDecision::Default(default_action.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::plugin_metadata::IpGroup;

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
    fn validate_structure_ok() {
        let groups = vec![IpGroup {
            name: "office".to_string(),
            description: None,
            cidrs: vec!["10.0.0.0/8".to_string()],
        }];
        assert!(validate_groups_structure(&groups).is_ok());
    }

    #[test]
    fn validate_structure_empty_errs() {
        assert!(validate_groups_structure(&[]).is_err());
    }

    #[test]
    fn validate_structure_duplicate_name_errs() {
        let groups = vec![
            IpGroup {
                name: "a".into(),
                description: None,
                cidrs: vec!["10.0.0.0/8".into()],
            },
            IpGroup {
                name: "a".into(),
                description: None,
                cidrs: vec!["192.168.0.0/16".into()],
            },
        ];
        let err = validate_groups_structure(&groups).unwrap_err();
        assert!(err.contains("duplicate group name"));
    }

    #[test]
    fn validate_structure_duplicate_cidr_errs() {
        let groups = vec![
            IpGroup {
                name: "a".into(),
                description: None,
                cidrs: vec!["10.0.0.0/8".into()],
            },
            IpGroup {
                name: "b".into(),
                description: None,
                cidrs: vec!["10.0.0.0/8".into()],
            },
        ];
        let err = validate_groups_structure(&groups).unwrap_err();
        assert!(err.contains("duplicate CIDR"));
    }

    #[test]
    fn validate_structure_bad_name_charset_errs() {
        let groups = vec![IpGroup {
            name: "bad name!".into(),
            description: None,
            cidrs: vec!["10.0.0.0/8".into()],
        }];
        assert!(validate_groups_structure(&groups).is_err());
    }

    #[test]
    fn validate_structure_long_name_errs() {
        let long = "a".repeat(64);
        let groups = vec![IpGroup {
            name: long,
            description: None,
            cidrs: vec!["10.0.0.0/8".into()],
        }];
        assert!(validate_groups_structure(&groups).is_err());
    }

    #[test]
    fn validate_structure_empty_cidrs_errs() {
        let groups = vec![IpGroup {
            name: "a".into(),
            description: None,
            cidrs: vec![],
        }];
        assert!(validate_groups_structure(&groups).is_err());
    }

    #[test]
    fn build_grouped_hits_label() {
        let groups = vec![IpGroup {
            name: "office".into(),
            description: None,
            cidrs: vec!["10.0.0.0/8".into()],
        }];
        let m = build_grouped_matcher(&groups).unwrap();
        let ip: std::net::IpAddr = "10.1.2.3".parse().unwrap();
        assert_eq!(m.matched_group(&ip), Some("office"));
    }

    #[test]
    fn validate_structure_single_char_name_ok() {
        let groups = vec![IpGroup {
            name: "a".into(),
            description: None,
            cidrs: vec!["10.0.0.0/8".into()],
        }];
        assert!(validate_groups_structure(&groups).is_ok());
    }

    #[test]
    fn validate_structure_two_char_name_ok() {
        let groups = vec![IpGroup {
            name: "ab".into(),
            description: None,
            cidrs: vec!["10.0.0.0/8".into()],
        }];
        assert!(validate_groups_structure(&groups).is_ok());
    }

    #[test]
    fn build_grouped_subnet_overlap_picks_longer_prefix() {
        let groups = vec![
            IpGroup {
                name: "wide".into(),
                description: None,
                cidrs: vec!["10.0.0.0/8".into()],
            },
            IpGroup {
                name: "narrow".into(),
                description: None,
                cidrs: vec!["10.0.0.0/24".into()],
            },
        ];
        let m = build_grouped_matcher(&groups).unwrap();
        let ip: std::net::IpAddr = "10.0.0.5".parse().unwrap();
        assert_eq!(m.matched_group(&ip), Some("narrow"));
        let outside: std::net::IpAddr = "10.5.0.0".parse().unwrap();
        assert_eq!(m.matched_group(&outside), Some("wide"));
    }
}
