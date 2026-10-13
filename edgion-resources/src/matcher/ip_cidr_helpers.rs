//! Shared IP / CIDR validation and IpRadixMatcher building helpers.
//! Used by HTTP and L4 IP-restriction plugins.

use std::collections::HashSet;
use std::net::IpAddr;

use crate::resources::edgion_config_data::{EdgionConfigDataRef, IpGroup};
use crate::resources::edgion_plugins::{AccessDecision, DefaultAction};
use edgion_algo::matcher::radix_ip::IpRadixMatcher;

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

/// One skipped/invalid entry on the partial-good compilation path. `group` is the
/// group name (empty string for list-level problems); `cidr` is the offending CIDR
/// (None for group-level / list-level problems); `reason` is an English message.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EntryError {
    pub group: String,
    pub cidr: Option<String>,
    pub reason: String,
}

impl std::fmt::Display for EntryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.cidr {
            Some(c) => write!(f, "group '{}' cidr '{}': {}", self.group, c, self.reason),
            None if self.group.is_empty() => write!(f, "{}", self.reason),
            None => write!(f, "group '{}': {}", self.group, self.reason),
        }
    }
}

/// Join a list of `EntryError`s into one `"; "`-separated message for a status condition.
pub fn format_entry_errors(errors: &[EntryError]) -> String {
    errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; ")
}

/// Partial-good sibling of [`validate_groups_structure`]: instead of failing on the
/// first problem, it returns EVERY skipped/invalid entry, keeping the valid ones
/// implicitly usable (the caller builds a matcher over what survives). Keep-first for
/// duplicate group names and cross-group duplicate CIDRs. An empty `groups` list is
/// VALID here (no error) — the all-or-nothing strict `validate_groups_structure` still
/// rejects it; this partial path does not. Pure: builds no matcher.
pub fn collect_entry_errors(groups: &[IpGroup]) -> Vec<EntryError> {
    let mut errors = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();
    let mut seen_cidrs: HashSet<String> = HashSet::new();
    for g in groups {
        if let Err(e) = validate_group_name(&g.name) {
            errors.push(EntryError {
                group: g.name.clone(),
                cidr: None,
                reason: format!("invalid group name: {}", e),
            });
            continue;
        }
        if !seen_names.insert(g.name.clone()) {
            errors.push(EntryError {
                group: g.name.clone(),
                cidr: None,
                reason: "duplicate group name, group skipped".to_string(),
            });
            continue;
        }
        if g.cidrs.is_empty() {
            errors.push(EntryError {
                group: g.name.clone(),
                cidr: None,
                reason: "group has no cidrs".to_string(),
            });
            continue;
        }
        for cidr in &g.cidrs {
            if let Err(e) = validate_ip_or_cidr(cidr) {
                errors.push(EntryError {
                    group: g.name.clone(),
                    cidr: Some(cidr.clone()),
                    reason: format!("invalid CIDR: {}", e),
                });
                continue;
            }
            let norm = match to_cidr_format(cidr) {
                Ok(n) => n,
                Err(e) => {
                    errors.push(EntryError {
                        group: g.name.clone(),
                        cidr: Some(cidr.clone()),
                        reason: format!("invalid CIDR: {}", e),
                    });
                    continue;
                }
            };
            if !seen_cidrs.insert(norm) {
                errors.push(EntryError {
                    group: g.name.clone(),
                    cidr: Some(cidr.clone()),
                    reason: "duplicate CIDR, dropped".to_string(),
                });
            }
        }
    }
    errors
}

/// Partial-good sibling of [`build_grouped_matcher`]: build a matcher over every
/// VALID entry, skipping (and returning) the invalid ones via [`EntryError`]. Never
/// fails — an all-bad or empty input yields an empty matcher that matches nothing
/// (load-bearing for intent-based whitelist fail-closed; see spec §7).
/// keep-first for duplicate group names and cross-group duplicate CIDRs.
///
/// Skip decisions are derived entirely from [`collect_entry_errors`] — no predicate
/// logic is duplicated. The returned `Vec<EntryError>` is exactly the output of that
/// function (plus a rare list-level error if the underlying builder itself fails).
pub fn build_grouped_matcher_partial(groups: &[IpGroup]) -> (IpRadixMatcher, Vec<EntryError>) {
    let errors = collect_entry_errors(groups);

    // The builder re-derives its skip decisions inline using the SAME seen-sets and
    // gating order as collect_entry_errors, rather than a value-keyed skip set built
    // from `errors`. A value-keyed skip set keyed by (group, cidr-string) is lossy:
    // when a single group lists the same CIDR string twice, collect_entry_errors
    // reports exactly ONE duplicate error (keep-first), but a single skip tuple would
    // match BOTH loop iterations and drop the entry entirely (silent fail-open for a
    // deny list). Mirroring the seen-sets guarantees the builder drops EXACTLY what
    // collect_entry_errors reports.
    let mut builder = IpRadixMatcher::builder();
    let mut seen_names: HashSet<&str> = HashSet::new();
    // Function-level seen-set over NORMALIZED CIDRs, declared at the SAME scope as
    // collect_entry_errors's `seen_cidrs` so duplicate gating (both intra-group and
    // cross-group, keep-first) matches that function exactly.
    let mut seen_cidrs: HashSet<String> = HashSet::new();
    let mut any_inserted = false;
    for g in groups {
        // Mirror collect_entry_errors's gating order so the set of compiled entries
        // exactly matches what that function considers valid.
        if validate_group_name(&g.name).is_err() {
            continue;
        }
        // Keep-first for duplicate names: skip any occurrence after the first.
        if !seen_names.insert(g.name.as_str()) {
            continue;
        }
        if g.cidrs.is_empty() {
            continue;
        }
        for cidr in &g.cidrs {
            // Invalid CIDR: already reported by collect_entry_errors, skip.
            if validate_ip_or_cidr(cidr).is_err() {
                continue;
            }
            let Ok(norm) = to_cidr_format(cidr) else {
                continue; // also already reported as invalid CIDR
            };
            // Keep-first duplicate drop: skip any occurrence after the first.
            if !seen_cidrs.insert(norm.clone()) {
                continue;
            }
            // insert() returns DuplicateCidr for host-bit duplicates (e.g. 10.0.0.0/8
            // vs 10.0.0.1/8): to_cidr_format does NOT zero host bits, so both reach the
            // radix builder with the same normalized prefix. That is benign — the
            // already-kept entry covers the same prefix, so the radix builder silently
            // coalesces it. insert() can otherwise fail only for a TooManyGroups
            // overflow; silently ignore both edge cases.
            let _ = builder.insert(&norm, &g.name);
            any_inserted = true;
        }
    }

    if !any_inserted {
        // All entries were skipped or input was empty — return infallible empty matcher.
        return (IpRadixMatcher::empty(), errors);
    }

    match builder.build() {
        Ok(m) => (m, errors),
        Err(e) => {
            // Rare hard failure (e.g. TooManyGroups overflow at build time); append a
            // list-level error so the caller still receives a usable Some(matcher).
            let mut errs = errors;
            errs.push(EntryError {
                group: String::new(),
                cidr: None,
                reason: format!("Failed to build IpRadixMatcher: {}", e),
            });
            (IpRadixMatcher::empty(), errs)
        }
    }
}

/// Structural validation of an `EdgionConfigDataRef` list: each ref must carry a
/// non-blank `name`. Existence/type of the referenced resource is NOT checked here
/// (that is a runtime/status concern resolved against the live store). Safe for the
/// admission hot path.
pub fn validate_refs_structure(refs: &[EdgionConfigDataRef]) -> Result<(), String> {
    for r in refs {
        if r.name.trim().is_empty() {
            return Err("EdgionConfigData reference has empty name".to_string());
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

/// Ordered borrowed matcher evaluation shared by every IP restriction consumer.
/// `deny`/`allow` lazily visit canonical matchers in authored reference order.
/// `allow_mode` follows non-empty authored allowRefs, independently of availability:
/// an empty available allow union still denies every address as a whitelist miss.
pub fn evaluate_ip_access_multi<'deny, 'allow>(
    deny: impl IntoIterator<Item = &'deny IpRadixMatcher>,
    allow: impl IntoIterator<Item = &'allow IpRadixMatcher>,
    allow_mode: bool,
    default_action: &DefaultAction,
    ip: &IpAddr,
) -> AccessDecision {
    for d in deny {
        if let Some(g) = d.matched_group_arc(ip) {
            return AccessDecision::DenyByGroup(g);
        }
    }
    if allow_mode {
        for a in allow {
            if let Some(g) = a.matched_group_arc(ip) {
                return AccessDecision::AllowByGroup(g);
            }
        }
        return AccessDecision::DenyByWhitelistMiss;
    }
    AccessDecision::Default(default_action.clone())
}

#[cfg(test)]
#[path = "../../tests/unit/matcher/ip_cidr_helpers_test.rs"]
mod tests;
