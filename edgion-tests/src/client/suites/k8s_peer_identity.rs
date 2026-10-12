//! Validated physical peer identity supplied by the Kubernetes harness.

use edgion_resources::matcher::ip_cidr_helpers::build_grouped_matcher;
use edgion_resources::resources::edgion_config_data::IpGroup;
use std::net::IpAddr;

pub(crate) struct PeerIdentity {
    pub ip: IpAddr,
    hit: String,
    miss: String,
}

fn env(name: &str) -> Result<String, String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("Missing prepared Kubernetes peer identity: {name}"))
}

fn contains(cidr: &str, ip: &IpAddr) -> Result<bool, String> {
    let matcher = build_grouped_matcher(&[IpGroup {
        name: "peer".into(),
        description: None,
        cidrs: vec![cidr.into()],
    }])?;
    Ok(matcher.contains(ip))
}

pub(crate) fn current() -> Result<PeerIdentity, String> {
    let uid = env("EDGION_TEST_K8S_PEER_UID")?;
    if uid.chars().any(char::is_whitespace) {
        return Err("Invalid prepared Kubernetes peer UID".into());
    }
    let ip = env("EDGION_TEST_K8S_PEER_IP")?
        .parse::<IpAddr>()
        .map_err(|error| format!("Invalid prepared Kubernetes peer IP: {error}"))?;
    let hit = env("EDGION_TEST_K8S_PEER_HIT_CIDR")?;
    let miss = env("EDGION_TEST_K8S_PEER_MISS_CIDR")?;
    let prefix = if ip.is_ipv4() { 32 } else { 128 };
    if hit != format!("{ip}/{prefix}") || !contains(&hit, &ip)? || contains(&miss, &ip)? {
        return Err("Prepared Kubernetes peer CIDRs do not prove exact hit and miss".into());
    }
    Ok(PeerIdentity { ip, hit, miss })
}

/// Only the explicit K8s CIDR template literals change; country assertions stay.
pub(crate) fn render_geo_template(template: &str) -> Result<String, String> {
    let peer = current()?;
    // Canonical MMDB maps 10/8 to US; the K8s extension maps OrbStack 192.168/16
    // to the same record. CIDR support alone does not prove IPv6 country support.
    if !contains("10.0.0.0/8", &peer.ip)? && !contains("192.168.0.0/16", &peer.ip)? {
        return Err("Kubernetes peer is outside the reviewed US GeoIP fixture ranges".into());
    }
    Ok(template
        .replace("\"192.168.0.0/16\"", &format!("\"{}\"", peer.hit))
        .replace("\"10.0.0.0/8\"", &format!("\"{}\"", peer.miss)))
}
