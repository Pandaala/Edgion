//! Shared geo rule-group matching primitives for `GeoIpLocation`.
//!
//! `GeoIpLocation` is one plugin registered into both `EdgionPlugin` (HTTP) and
//! `EdgionStreamPlugin` (L4). This module holds the config shape and the pure
//! matching semantics; it has no dependency on either plugin layer or on the
//! gateway's `geoip` module (no MMDB, no `GeoLookup`). The gateway resolves a
//! `GeoLookup` into the plain `Option<&str>`/`IpAddr` facts this module
//! consumes, so the matching logic itself stays testable with no MMDB fixture.
//!
//! # Matching model
//!
//! A [`GeoRuleGroup`] is a list of [`GeoRule`]s, ANDed together. A [`GeoRule`]
//! answers one boolean question against ONE dimension of ONE address:
//!
//! 1. **An empty `values` list is a hard admission error.** A rule with no
//!    values expresses no criterion, so there is no correct thing for it to
//!    mean: read as "unconstrained" it silently matches everything, which
//!    turns a deny group into a total outage and an allow group into an open
//!    whitelist — a coin flip between an incident and a breach. Rejecting it
//!    at admission removes the choice. Should one ever reach the matcher
//!    anyway (a struct built in Rust, bypassing deserialization), it matches
//!    NOTHING, consistent with the `cidr` arm, which already yields `false`
//!    when no matcher was compiled.
//! 2. **A dimension the record cannot supply makes the rule NOT match.**
//!    Absence is ordinary missingness, not an error — the same rule the
//!    HTTP-side `RestrictionRule` applies via `onMissing`, simplified here to
//!    a fixed "absent => false" because `GeoRule` has no `onMissing` knob.
//!
//! Every struct here is `deny_unknown_fields`: this is a security control, and
//! a silently-swallowed typo (`denyRuleGroup` for `denyRuleGroups`,
//! `valuez` for `values`) degrades it to a no-op while the resource still
//! reports `Accepted: True`. Serde is the only gate that can catch this — the
//! EdgionPlugins CRD models plugin `config` as
//! `x-kubernetes-preserve-unknown-fields`, so the API server passes unknown
//! keys straight through.
//!
//! Groups are ORed: `denies()`/`allows()` are true iff ANY group matches. The
//! plugin-level decision (deny-first, then allow-miss, then `defaultAction`) is
//! [`GeoIpLocationConfig::decide`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

use crate::matcher::ip_radix_tree::types::IpCidr;
use crate::matcher::ip_radix_tree::{IpRadixMatcher, IpRadixMatcherBuilder};

/// Data source for a geo rule's underlying peer IP.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum GeoRuleSource {
    /// The upstream-hop direct TCP peer. Un-forgeable. Valid at both L4 and L7.
    DirectPeerIp,
    /// The XFF-resolved real client. Forgeable by anything not behind a
    /// trusted proxy chain. **L7 only** — L4 has no HTTP headers to parse a
    /// forwarded-for chain from; a rule using this source in an L4
    /// `GeoIpLocation` config is a hard admission error.
    RemoteIp,
}

/// The matchable dimension a [`GeoRule`] targets.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GeoRuleKey {
    /// Match the address itself against one or more CIDRs/IPs.
    Cidr,
    /// ISO 3166-1 alpha-2 country code, uppercased at compile time.
    Country,
    /// ISO continent code (`AF`/`AN`/`AS`/`EU`/`NA`/`OC`/`SA`), uppercased at compile time.
    Continent,
    /// City name, matched against the record's `names["en"]`. Free text, case-sensitive.
    City,
    /// ISO 3166-2 subdivision code, uppercased at compile time.
    Subdivision,
}

/// One AND-ed predicate: does `source`'s resolved `key` dimension fall in `values`?
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GeoRule {
    pub source: GeoRuleSource,
    pub key: GeoRuleKey,

    /// Must be non-empty (see this module's matching-model note #1). For
    /// `key: cidr`, each entry must be a valid CIDR or bare IP address.
    #[serde(default)]
    pub values: Vec<String>,

    /// Compiled CIDR matcher for `key: cidr`. Runtime-only.
    #[serde(skip)]
    #[schemars(skip)]
    pub cidr_matcher: Option<IpRadixMatcher>,
}

/// One OR-ed group: every rule in `rules` must match (AND).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GeoRuleGroup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    #[serde(default)]
    pub rules: Vec<GeoRule>,
}

/// Resolved dimension facts for ONE address (already-uppercased for
/// country/continent/subdivision, matching [`GeoRule::compile`]'s
/// normalization so a straight `==` comparison is correct).
#[derive(Debug, Clone, Copy, Default)]
pub struct GeoDimFacts<'a> {
    pub country: Option<&'a str>,
    pub continent: Option<&'a str>,
    pub subdivision: Option<&'a str>,
    pub city: Option<&'a str>,
}

/// Everything a [`GeoRuleGroup`] needs to evaluate: the peer IP and resolved
/// dimensions for BOTH sources. A caller that never consults `RemoteIp` rules
/// (every valid L4 config) may pass `remote_ip: None` /
/// `remote: GeoDimFacts::default()` — those fields are simply never read.
#[derive(Debug, Clone, Copy, Default)]
pub struct GeoRuleContext<'a> {
    pub direct_peer_ip: Option<IpAddr>,
    pub direct_peer: GeoDimFacts<'a>,
    pub remote_ip: Option<IpAddr>,
    pub remote: GeoDimFacts<'a>,
}

/// Parse a config CIDR/IP value into a normalized CIDR string (`x/y`), so a
/// bare IP (`"10.0.0.1"`) is accepted as shorthand for its /32 (or /128).
fn normalize_cidr_or_ip(value: &str) -> Result<String, String> {
    if IpCidr::parse(value).is_ok() {
        return Ok(value.to_string());
    }
    match value.parse::<IpAddr>() {
        Ok(IpAddr::V4(_)) => Ok(format!("{value}/32")),
        Ok(IpAddr::V6(_)) => Ok(format!("{value}/128")),
        Err(_) => Err(format!("'{value}' is not a valid CIDR or IP address")),
    }
}

impl GeoRule {
    /// Stateless per-rule field validation. `None` = valid.
    ///
    /// `is_stream` is the ONLY place layer awareness enters this shared
    /// module: an L4 (`is_stream: true`) config using `RemoteIp` is a hard
    /// admission error because one stream resource may run before PROXY
    /// resolution, where only DirectPeerIp is uniformly available.
    pub fn field_error(&self, is_stream: bool) -> Option<String> {
        if is_stream && self.source == GeoRuleSource::RemoteIp {
            return Some(
                "source RemoteIp is not valid in an L4 (stream) GeoIpLocation config because the \
                 resource may execute before PROXY resolution; use uniformly available DirectPeerIp"
                    .to_string(),
            );
        }
        if self.values.is_empty() {
            return Some(
                "values must not be empty — a rule with no values expresses no criterion; \
                 remove the rule instead"
                    .to_string(),
            );
        }
        if self.key == GeoRuleKey::Cidr {
            for v in &self.values {
                if let Err(e) = normalize_cidr_or_ip(v) {
                    return Some(e);
                }
            }
        }
        None
    }

    /// Compile: validate fields, uppercase ISO-coded values, WARN (never
    /// reject) on an unrecognized country/continent code, and build the CIDR
    /// matcher for `key: cidr`.
    pub fn compile(&mut self, is_stream: bool) -> Result<(), String> {
        if let Some(e) = self.field_error(is_stream) {
            return Err(e);
        }
        match self.key {
            GeoRuleKey::Cidr => {
                if !self.values.is_empty() {
                    let mut builder = IpRadixMatcherBuilder::new();
                    for v in &self.values {
                        let normalized = normalize_cidr_or_ip(v)?;
                        builder.insert(&normalized, "m").map_err(|e| e.to_string())?;
                    }
                    self.cidr_matcher = Some(builder.build().map_err(|e| e.to_string())?);
                }
            }
            GeoRuleKey::Country => {
                for v in self.values.iter_mut() {
                    *v = v.to_ascii_uppercase();
                }
                for v in &self.values {
                    if !crate::geoip_codes::is_valid_country(v) {
                        tracing::warn!("GeoIpLocation rule: '{}' is not a known country code", v);
                    }
                }
            }
            GeoRuleKey::Continent => {
                for v in self.values.iter_mut() {
                    *v = v.to_ascii_uppercase();
                }
                for v in &self.values {
                    if !crate::geoip_codes::is_valid_continent(v) {
                        tracing::warn!("GeoIpLocation rule: '{}' is not a known continent code", v);
                    }
                }
            }
            GeoRuleKey::Subdivision => {
                for v in self.values.iter_mut() {
                    *v = v.to_ascii_uppercase();
                }
            }
            // City is free text; kept as-is (case-sensitive, matches record `names["en"]`).
            GeoRuleKey::City => {}
        }
        Ok(())
    }

    fn dimension_matches(values: &[String], value: Option<&str>) -> bool {
        match value {
            Some(v) => values.iter().any(|x| x == v),
            None => false,
        }
    }

    /// Evaluate this rule against the facts for its OWN source (the caller
    /// picks `ip`/`facts` from [`GeoRuleContext`] by `self.source`).
    pub fn matches(&self, ip: Option<IpAddr>, facts: &GeoDimFacts<'_>) -> bool {
        // No early "empty => always matches" case: validation rejects an empty
        // `values`, and every arm below already yields `false` for it.
        match self.key {
            GeoRuleKey::Cidr => match (ip, &self.cidr_matcher) {
                (Some(ip), Some(matcher)) => matcher.contains(&ip),
                _ => false,
            },
            GeoRuleKey::Country => Self::dimension_matches(&self.values, facts.country),
            GeoRuleKey::Continent => Self::dimension_matches(&self.values, facts.continent),
            GeoRuleKey::City => Self::dimension_matches(&self.values, facts.city),
            GeoRuleKey::Subdivision => Self::dimension_matches(&self.values, facts.subdivision),
        }
    }
}

impl GeoRuleGroup {
    pub fn check_fields(&self, is_stream: bool) -> Option<String> {
        if self.rules.is_empty() {
            return Some("a rule group must have at least one rule".to_string());
        }
        for (ri, rule) in self.rules.iter().enumerate() {
            if let Some(e) = rule.field_error(is_stream) {
                return Some(format!("rules[{ri}]: {e}"));
            }
        }
        None
    }

    pub fn compile(&mut self, is_stream: bool) -> Result<(), String> {
        if let Some(e) = self.check_fields(is_stream) {
            return Err(e);
        }
        for (ri, rule) in self.rules.iter_mut().enumerate() {
            rule.compile(is_stream).map_err(|e| format!("rules[{ri}]: {e}"))?;
        }
        Ok(())
    }

    /// AND over every rule, each pulling its facts from `ctx` by its own `source`.
    pub fn matches(&self, ctx: &GeoRuleContext<'_>) -> bool {
        self.rules.iter().all(|rule| {
            let (ip, facts) = match rule.source {
                GeoRuleSource::DirectPeerIp => (ctx.direct_peer_ip, &ctx.direct_peer),
                GeoRuleSource::RemoteIp => (ctx.remote_ip, &ctx.remote),
            };
            rule.matches(ip, facts)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(source: GeoRuleSource, key: GeoRuleKey, values: &[&str]) -> GeoRule {
        GeoRule {
            source,
            key,
            values: values.iter().map(|s| s.to_string()).collect(),
            cidr_matcher: None,
        }
    }

    fn facts(country: Option<&str>) -> GeoDimFacts<'_> {
        GeoDimFacts {
            country,
            ..Default::default()
        }
    }

    /// An empty `values` is rejected at admission, and — if one is built in
    /// Rust anyway — matches nothing rather than everything. The old
    /// "empty means unconstrained" reading made a mistyped `valuez:` silently
    /// match every request: a total outage in a deny group, an open whitelist
    /// in an allow group.
    #[test]
    fn empty_values_is_rejected_and_matches_nothing() {
        let mut r = rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &[]);
        let err = r.field_error(false).expect("empty values must be rejected");
        assert!(err.contains("values must not be empty"), "unexpected error: {err}");
        assert!(r.compile(false).is_err());
        assert!(!r.matches(None, &facts(None)));
        assert!(!r.matches(None, &facts(Some("RU"))));
    }

    /// The same rejection applies to `key: cidr`, whose empty case previously
    /// took the very same "unconstrained" shortcut.
    #[test]
    fn empty_values_is_rejected_for_cidr_too() {
        let r = rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Cidr, &[]);
        assert!(r.field_error(false).is_some());
    }

    /// A mistyped field name must not be swallowed: the whole point of
    /// `deny_unknown_fields` on these structs. `valuez` used to deserialize
    /// into an empty `values`, silently neutering the rule.
    #[test]
    fn unknown_field_in_rule_is_rejected() {
        let e = serde_yaml::from_str::<GeoRule>("source: DirectPeerIp\nkey: country\nvaluez: [\"RU\"]\n")
            .expect_err("a typo'd field must be rejected, not swallowed");
        assert!(e.to_string().contains("valuez"), "unexpected error: {e}");
    }

    #[test]
    fn unknown_field_in_group_is_rejected() {
        assert!(
            serde_yaml::from_str::<GeoRuleGroup>("name: g\nrulez: []\n").is_err(),
            "a typo'd group field must be rejected"
        );
    }

    #[test]
    fn missing_dimension_does_not_match_a_constrained_rule() {
        let mut r = rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["RU"]);
        r.compile(false).unwrap();
        assert!(!r.matches(None, &facts(None)));
    }

    #[test]
    fn country_uppercased_at_compile() {
        let mut r = rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["ru"]);
        r.compile(false).unwrap();
        assert_eq!(r.values, vec!["RU".to_string()]);
        assert!(r.matches(None, &facts(Some("RU"))));
    }

    #[test]
    fn cidr_matches_by_membership() {
        let mut r = rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Cidr, &["10.0.0.0/8"]);
        r.compile(false).unwrap();
        let ip: IpAddr = "10.1.2.3".parse().unwrap();
        assert!(r.matches(Some(ip), &GeoDimFacts::default()));
        let other: IpAddr = "192.168.1.1".parse().unwrap();
        assert!(!r.matches(Some(other), &GeoDimFacts::default()));
    }

    #[test]
    fn bare_ip_is_accepted_as_cidr_shorthand() {
        let mut r = rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Cidr, &["203.0.113.5"]);
        r.compile(false).unwrap();
        let ip: IpAddr = "203.0.113.5".parse().unwrap();
        assert!(r.matches(Some(ip), &GeoDimFacts::default()));
    }

    #[test]
    fn invalid_cidr_is_rejected() {
        let r = rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Cidr, &["not-an-ip"]);
        assert!(r.field_error(false).is_some());
    }

    #[test]
    fn remote_ip_is_rejected_in_l4_context() {
        let r = rule(GeoRuleSource::RemoteIp, GeoRuleKey::Country, &["RU"]);
        assert!(r.field_error(true).unwrap().contains("RemoteIp"));
        assert!(r.field_error(false).is_none());
    }

    #[test]
    fn group_is_and_across_rules() {
        let mut group = GeoRuleGroup {
            name: Some("moscow".into()),
            rules: vec![
                rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::City, &["Moscow"]),
                rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["ru"]),
            ],
        };
        group.compile(false).unwrap();

        let ctx_hit = GeoRuleContext {
            direct_peer: GeoDimFacts {
                country: Some("RU"),
                city: Some("Moscow"),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(group.matches(&ctx_hit));

        // Country matches but city doesn't -> AND fails.
        let ctx_miss = GeoRuleContext {
            direct_peer: GeoDimFacts {
                country: Some("RU"),
                city: Some("Kazan"),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!group.matches(&ctx_miss));
    }

    #[test]
    fn empty_rule_group_is_rejected() {
        let g = GeoRuleGroup {
            name: None,
            rules: vec![],
        };
        assert!(g.check_fields(false).is_some());
    }
}
