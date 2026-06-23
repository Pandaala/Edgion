//! Shared IP / CIDR validation and IpRadixMatcher building helpers.
//! Used by HTTP and L4 IP-restriction plugins.

use std::collections::HashSet;
use std::net::IpAddr;

use crate::matcher::ip_radix_tree::IpRadixMatcher;
use crate::resources::edgion_config_data::{EdgionConfigDataRef, IpGroup};
use crate::resources::edgion_plugins::{AccessDecision, DefaultAction};

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
            let norm = match to_cidr_format(cidr) {
                Ok(n) => n,
                Err(_) => continue, // also already reported as invalid CIDR
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

/// Single-matcher **compatibility shim** over [`evaluate_ip_access_multi`].
///
/// Whitelist mode here is decided by **content** (`allow.is_some()`), NOT by
/// configured intent. That is safe only because the partial builder always yields
/// `Some(empty)` for a present-but-all-bad allow list (Task 2), so a configured
/// whitelist can never silently collapse into "no allow matcher → default-allow".
/// Even so, **live whitelist evaluation MUST use the intent-based
/// [`evaluate_ip_access_multi`]** (via `check_ip_access_with_refs` /
/// `check_ip_with_refs`), which derives `allow_mode` from configured intent and
/// folds in reference-resolved matchers. This shim is retained only for the
/// single-matcher unit tests and the ref-less `check_ip`/`check_ip_access`
/// convenience wrappers; do not introduce new live whitelist call sites here.
///
/// Behaviour (canonical deny > allow > default priority used by every
/// IP-restriction plugin — HTTP `IpRestrictionConfig`, L4
/// `ConnectionIpRestrictionRules`, cluster-scope `GlobalConnectionIpRestriction`):
/// - `deny` match wins → `DenyByGroup(g)`.
/// - `allow` is present and matches → `AllowByGroup(g)`.
/// - `allow` is present but does not match → `DenyByWhitelistMiss`.
/// - `allow` is absent → fall through to `Default(default_action.clone())`.
///
/// Keeping the algorithm in one place prevents the call sites from silently
/// diverging when the priority or returned variants change. See spec §4.6 truth
/// table.
pub fn evaluate_ip_access(
    deny: Option<&IpRadixMatcher>,
    allow: Option<&IpRadixMatcher>,
    default_action: &DefaultAction,
    ip: &IpAddr,
) -> AccessDecision {
    // Preserve the historical semantics exactly: "allow present" == whitelist mode.
    let deny_slice: Vec<&IpRadixMatcher> = deny.into_iter().collect();
    let allow_slice: Vec<&IpRadixMatcher> = allow.into_iter().collect();
    evaluate_ip_access_multi(&deny_slice, &allow_slice, allow.is_some(), default_action, ip)
}

/// N-matcher generalization of [`evaluate_ip_access`]. `deny`/`allow` are the
/// union of inline and reference-resolved matchers; `allow_mode` is whitelist
/// mode decided by *configured intent* (inline allow non-empty OR allow_refs
/// configured non-empty), NOT by whether any allow matcher resolved. See spec
/// Decisions 4 & 5: an allow-mode config with an empty resolved allow set
/// denies all (fail-closed whitelist).
pub fn evaluate_ip_access_multi(
    deny: &[&IpRadixMatcher],
    allow: &[&IpRadixMatcher],
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
mod tests {
    use super::*;
    use crate::resources::edgion_config_data::IpGroup;

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

    #[test]
    fn multi_deny_any_matcher_wins() {
        let d1 = build_grouped_matcher(&[IpGroup {
            name: "a".into(),
            description: None,
            cidrs: vec!["1.1.1.1/32".into()],
        }])
        .unwrap();
        let d2 = build_grouped_matcher(&[IpGroup {
            name: "b".into(),
            description: None,
            cidrs: vec!["2.2.2.2/32".into()],
        }])
        .unwrap();
        let ip: IpAddr = "2.2.2.2".parse().unwrap();
        let dec = evaluate_ip_access_multi(&[&d1, &d2], &[], false, &DefaultAction::Allow, &ip);
        assert!(matches!(dec, AccessDecision::DenyByGroup(g) if &*g == "b"));
    }

    #[test]
    fn multi_allow_mode_miss_is_whitelist_miss() {
        let a = build_grouped_matcher(&[IpGroup {
            name: "office".into(),
            description: None,
            cidrs: vec!["192.168.0.0/16".into()],
        }])
        .unwrap();
        let ip: IpAddr = "8.8.8.8".parse().unwrap();
        let dec = evaluate_ip_access_multi(&[], &[&a], true, &DefaultAction::Allow, &ip);
        assert!(matches!(dec, AccessDecision::DenyByWhitelistMiss));
    }

    #[test]
    fn multi_allow_mode_true_with_empty_allow_denies_all() {
        // allowRefs-configured-but-all-missing case: allow_mode=true, no allow matchers.
        let ip: IpAddr = "8.8.8.8".parse().unwrap();
        let dec = evaluate_ip_access_multi(&[], &[], true, &DefaultAction::Allow, &ip);
        assert!(matches!(dec, AccessDecision::DenyByWhitelistMiss));
    }

    #[test]
    fn multi_not_allow_mode_falls_through_to_default() {
        let ip: IpAddr = "8.8.8.8".parse().unwrap();
        let dec = evaluate_ip_access_multi(&[], &[], false, &DefaultAction::Deny, &ip);
        assert!(matches!(dec, AccessDecision::Default(DefaultAction::Deny)));
    }

    #[test]
    fn multi_allow_union_across_matchers() {
        let a1 = build_grouped_matcher(&[IpGroup {
            name: "g1".into(),
            description: None,
            cidrs: vec!["10.0.0.0/8".into()],
        }])
        .unwrap();
        let a2 = build_grouped_matcher(&[IpGroup {
            name: "g2".into(),
            description: None,
            cidrs: vec!["192.168.0.0/16".into()],
        }])
        .unwrap();
        let ip: IpAddr = "192.168.1.1".parse().unwrap();
        let dec = evaluate_ip_access_multi(&[], &[&a1, &a2], true, &DefaultAction::Deny, &ip);
        assert!(matches!(dec, AccessDecision::AllowByGroup(g) if &*g == "g2"));
    }

    #[test]
    fn refs_structure_ok() {
        use crate::resources::edgion_config_data::EdgionConfigDataRef;
        let refs = vec![EdgionConfigDataRef {
            name: "corp".into(),
            namespace: None,
            ..Default::default()
        }];
        assert!(validate_refs_structure(&refs).is_ok());
    }

    #[test]
    fn refs_structure_empty_name_errs() {
        use crate::resources::edgion_config_data::EdgionConfigDataRef;
        let refs = vec![EdgionConfigDataRef {
            name: "  ".into(),
            namespace: None,
            ..Default::default()
        }];
        assert!(validate_refs_structure(&refs).is_err());
    }

    #[test]
    fn refs_structure_empty_list_ok() {
        assert!(validate_refs_structure(&[]).is_ok());
    }

    #[test]
    fn collect_errors_empty_list_is_ok() {
        // Empty items is valid (NOT an error) on the partial path.
        assert!(collect_entry_errors(&[]).is_empty());
    }

    #[test]
    fn collect_errors_invalid_cidr_reported_rest_kept() {
        let groups = vec![IpGroup {
            name: "g".into(),
            description: None,
            cidrs: vec!["10.0.0.0/8".into(), "not-an-ip".into()],
        }];
        let errs = collect_entry_errors(&groups);
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].group, "g");
        assert_eq!(errs[0].cidr.as_deref(), Some("not-an-ip"));
        assert!(errs[0].reason.contains("invalid CIDR"));
    }

    #[test]
    fn collect_errors_empty_group_reported() {
        let groups = vec![IpGroup {
            name: "g".into(),
            description: None,
            cidrs: vec![],
        }];
        let errs = collect_entry_errors(&groups);
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].group, "g");
        assert!(errs[0].cidr.is_none());
        assert!(errs[0].reason.contains("no cidrs"));
    }

    #[test]
    fn collect_errors_duplicate_cidr_keep_first() {
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
        let errs = collect_entry_errors(&groups);
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].group, "b");
        assert_eq!(errs[0].cidr.as_deref(), Some("10.0.0.0/8"));
        assert!(errs[0].reason.contains("duplicate CIDR"));
    }

    #[test]
    fn collect_errors_duplicate_group_name_keep_first() {
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
        let errs = collect_entry_errors(&groups);
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].group, "a");
        assert!(errs[0].reason.contains("duplicate group name"));
    }

    #[test]
    fn collect_errors_invalid_group_name() {
        let groups = vec![IpGroup {
            name: "bad name!".into(),
            description: None,
            cidrs: vec!["10.0.0.0/8".into()],
        }];
        let errs = collect_entry_errors(&groups);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].reason.contains("invalid group name"));
    }

    #[test]
    fn format_entry_errors_lists_each() {
        let errs = vec![
            EntryError {
                group: "a".into(),
                cidr: Some("x".into()),
                reason: "invalid CIDR: x".into(),
            },
            EntryError {
                group: "b".into(),
                cidr: None,
                reason: "group has no cidrs".into(),
            },
        ];
        let s = format_entry_errors(&errs);
        assert!(s.contains("a"));
        assert!(s.contains("invalid CIDR"));
        assert!(s.contains("b"));
        assert!(s.contains("; "));
    }

    #[test]
    fn partial_one_bad_cidr_keeps_the_rest() {
        let groups = vec![IpGroup {
            name: "office".into(),
            description: None,
            cidrs: vec!["10.0.0.0/8".into(), "not-an-ip".into()],
        }];
        let (m, errs) = build_grouped_matcher_partial(&groups);
        assert_eq!(errs.len(), 1);
        let ip: std::net::IpAddr = "10.1.2.3".parse().unwrap();
        assert_eq!(m.matched_group(&ip), Some("office")); // good CIDR survived
    }

    #[test]
    fn partial_empty_group_skipped_others_survive() {
        let groups = vec![
            IpGroup {
                name: "empty".into(),
                description: None,
                cidrs: vec![],
            },
            IpGroup {
                name: "good".into(),
                description: None,
                cidrs: vec!["192.168.0.0/16".into()],
            },
        ];
        let (m, errs) = build_grouped_matcher_partial(&groups);
        assert_eq!(errs.len(), 1);
        let ip: std::net::IpAddr = "192.168.1.1".parse().unwrap();
        assert_eq!(m.matched_group(&ip), Some("good"));
    }

    #[test]
    fn partial_duplicate_cidr_keep_first_group() {
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
        let (m, errs) = build_grouped_matcher_partial(&groups);
        assert_eq!(errs.len(), 1);
        let ip: std::net::IpAddr = "10.1.2.3".parse().unwrap();
        assert_eq!(m.matched_group(&ip), Some("a")); // first occurrence wins
    }

    #[test]
    fn partial_empty_items_is_empty_matcher_no_error() {
        let (m, errs) = build_grouped_matcher_partial(&[]);
        assert!(errs.is_empty());
        let ip: std::net::IpAddr = "8.8.8.8".parse().unwrap();
        assert_eq!(m.matched_group(&ip), None); // Some(empty), matches nothing
    }

    #[test]
    fn partial_all_bad_is_empty_matcher_with_errors() {
        let groups = vec![IpGroup {
            name: "g".into(),
            description: None,
            cidrs: vec!["bad1".into(), "bad2".into()],
        }];
        let (m, errs) = build_grouped_matcher_partial(&groups);
        assert_eq!(errs.len(), 2);
        let ip: std::net::IpAddr = "8.8.8.8".parse().unwrap();
        assert_eq!(m.matched_group(&ip), None);
    }

    #[test]
    fn partial_duplicate_group_name_keeps_first() {
        // The first "foo" group is valid; the second is a duplicate and must be skipped.
        // The compiled matcher must still match IPs from the first group's CIDR.
        let groups = vec![
            IpGroup {
                name: "foo".into(),
                description: None,
                cidrs: vec!["10.0.0.0/8".into()],
            },
            IpGroup {
                name: "foo".into(),
                description: None,
                cidrs: vec!["192.168.0.0/16".into()],
            },
        ];
        let (m, errs) = build_grouped_matcher_partial(&groups);
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].group, "foo");
        assert!(errs[0].reason.contains("duplicate group name"));
        let ip: std::net::IpAddr = "10.1.2.3".parse().unwrap();
        assert_eq!(m.matched_group(&ip), Some("foo")); // first "foo" must survive
    }

    #[test]
    fn collect_errors_intra_group_duplicate_cidr_keep_first() {
        // A single group listing the same CIDR string twice: keep-first means exactly
        // ONE duplicate error (the 2nd occurrence), the first is kept.
        let groups = vec![IpGroup {
            name: "g".into(),
            description: None,
            cidrs: vec!["1.2.3.4/32".into(), "1.2.3.4/32".into()],
        }];
        let errs = collect_entry_errors(&groups);
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].group, "g");
        assert_eq!(errs[0].cidr.as_deref(), Some("1.2.3.4/32"));
        assert!(errs[0].reason.contains("duplicate CIDR"));
    }

    #[test]
    fn partial_intra_group_duplicate_cidr_keeps_first() {
        // Same input as above: the builder must drop EXACTLY what collect reports —
        // ONE duplicate, with the first occurrence KEPT (entry NOT vanished). A lossy
        // value-keyed skip set would drop both and silently produce an empty matcher.
        let groups = vec![IpGroup {
            name: "g".into(),
            description: None,
            cidrs: vec!["1.2.3.4/32".into(), "1.2.3.4/32".into()],
        }];
        let (m, errs) = build_grouped_matcher_partial(&groups);
        assert_eq!(errs.len(), 1);
        let ip: std::net::IpAddr = "1.2.3.4".parse().unwrap();
        assert_eq!(m.matched_group(&ip), Some("g")); // first occurrence KEPT
    }
}
