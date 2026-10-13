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
//! 1. **Every authored criterion must be nonempty.** Geographic keys require
//!    values; CIDR keys require ipRefs. Validate before runtime construction.
//!    CIDR membership is supplied by an already bound central IpList predicate;
//!    an available empty compiled list matches nothing. Private runtime copies
//!    may discard authored CIDR refs after binding and are never readmitted.
//! 2. **A dimension the record cannot supply makes the rule NOT match.**
//!    Absence is ordinary missingness, not an error. This follows the shared
//!    HTTP [`Condition`](crate::resources::edgion_plugins::conditions::Condition)
//!    model: a missing value gives `KeyMatch` or `keyExist` a base result of
//!    `false`, and `negate` is applied only afterward. `GeoRule` has no
//!    `negate` field, so its missing-dimension result remains `false`.
//!
//! Unknown and removed input fields produce a transported invalid generation.
//! Rejected values are discarded; a clean generation can recover.
//!
//! Groups are ORed: `denies()`/`allows()` are true iff ANY group matches. The
//! plugin-level decision (deny-first, then allow-miss, then `defaultAction`) is
//! [`GeoIpLocationConfig::decide`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

use crate::resources::edgion_config_data::EdgionConfigDataRef;
use crate::resources::edgion_plugins::ip_reference_set::{IpReadResolution, IpReferenceSet};
use serde::de::{IgnoredAny, SeqAccess, Visitor};
use std::collections::HashMap;

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
    #[serde(other)]
    #[schemars(skip)]
    Unsupported,
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
    #[serde(other)]
    #[schemars(skip)]
    Unsupported,
}

/// One predicate. CIDR criteria borrow centrally compiled IpLists.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GeoRule {
    pub source: GeoRuleSource,
    pub key: GeoRuleKey,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    #[serde(flatten)]
    pub cidr: IpReferenceSet,
}

/// One OR-ed group: every rule must match.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GeoRuleGroup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub rules: Vec<GeoRule>,
    #[serde(default, skip_serializing_if = "is_false")]
    #[schemars(skip)]
    pub invalid_input: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}

/// Presence is retained even for null or empty removed input. Payloads are cold-only.
#[derive(Default)]
struct ValuesWire {
    present: bool,
    invalid: bool,
    values: Vec<String>,
}
impl<'de> Deserialize<'de> for ValuesWire {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ValuesVisitor;
        impl<'de> Visitor<'de> for ValuesVisitor {
            type Value = ValuesWire;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a string list")
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(ValuesWire {
                    present: true,
                    invalid: true,
                    values: Vec::new(),
                })
            }
            fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                self.visit_unit()
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                #[derive(Deserialize)]
                #[serde(untagged)]
                enum Element {
                    Text(String),
                    Rejected(IgnoredAny),
                }
                let mut result = ValuesWire {
                    present: true,
                    ..Default::default()
                };
                while let Some(element) = seq.next_element::<Element>()? {
                    match element {
                        Element::Text(value) => result.values.push(value),
                        Element::Rejected(_) => result.invalid = true,
                    }
                }
                Ok(result)
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                self.visit_unit()
            }
            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self::Value, E> {
                self.visit_unit()
            }
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self::Value, E> {
                self.visit_unit()
            }
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Self::Value, E> {
                self.visit_unit()
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self::Value, E> {
                self.visit_unit()
            }
            fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Self::Value, E> {
                self.visit_unit()
            }
        }
        deserializer.deserialize_any(ValuesVisitor)
    }
}

#[derive(Default)]
struct RefsWire {
    present: bool,
    refs: Vec<EdgionConfigDataRef>,
}
impl<'de> Deserialize<'de> for RefsWire {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let refs = Option::<Vec<EdgionConfigDataRef>>::deserialize(deserializer)?.unwrap_or_default();
        Ok(Self { present: true, refs })
    }
}
impl<'de> Deserialize<'de> for GeoRule {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire {
            source: Option<GeoRuleSource>,
            key: Option<GeoRuleKey>,
            #[serde(default)]
            values: ValuesWire,
            #[serde(default)]
            ip_refs: RefsWire,
            #[serde(default)]
            invalid_input: bool,
            #[serde(default)]
            resolved_refs: Option<Box<IpReadResolution>>,
            #[serde(flatten)]
            rejected: HashMap<String, IgnoredAny>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let key = wire.key.unwrap_or(GeoRuleKey::Unsupported);
        let invalid = wire.invalid_input
            || !wire.rejected.is_empty()
            || wire.values.invalid
            || (key == GeoRuleKey::Cidr && wire.values.present)
            || (key != GeoRuleKey::Cidr && wire.ip_refs.present);
        Ok(Self {
            source: wire.source.unwrap_or(GeoRuleSource::Unsupported),
            key,
            values: if key == GeoRuleKey::Cidr {
                Vec::new()
            } else {
                wire.values.values
            },
            cidr: IpReferenceSet {
                ip_refs: wire.ip_refs.refs,
                invalid_input: invalid,
                resolved_refs: wire.resolved_refs,
            },
        })
    }
}
impl<'de> Deserialize<'de> for GeoRuleGroup {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire {
            name: Option<String>,
            #[serde(default)]
            rules: Vec<GeoRule>,
            #[serde(default)]
            invalid_input: bool,
            #[serde(flatten)]
            rejected: HashMap<String, IgnoredAny>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self {
            name: wire.name,
            rules: wire.rules,
            invalid_input: wire.invalid_input || !wire.rejected.is_empty(),
        })
    }
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

impl GeoRule {
    /// Stateless per-rule field validation. `None` = valid.
    ///
    /// `is_stream` is the ONLY place layer awareness enters this shared
    /// module: an L4 (`is_stream: true`) config using `RemoteIp` is a hard
    /// admission error because one stream resource may run before PROXY
    /// resolution, where only DirectPeerIp is uniformly available.
    pub fn field_error(&self, is_stream: bool) -> Option<String> {
        if self.cidr.invalid_input || self.source == GeoRuleSource::Unsupported || self.key == GeoRuleKey::Unsupported {
            return Some("GeoIpLocation rule contains unsupported or removed fields".into());
        }
        if is_stream && self.source == GeoRuleSource::RemoteIp {
            return Some(
                "source RemoteIp is not valid in an L4 (stream) GeoIpLocation config; use DirectPeerIp".into(),
            );
        }
        if self.key == GeoRuleKey::Cidr {
            if !self.values.is_empty() {
                return Some("CIDR rules accept ipRefs only".into());
            }
            return self.cidr.validate().err();
        }
        if !self.cidr.ip_refs.is_empty() || self.values.is_empty() {
            return Some("Geographic rules require nonempty values and do not accept ipRefs".into());
        }
        None
    }

    /// Compile: validate fields, uppercase ISO-coded values, WARN (never
    /// reject) on an unrecognized country/continent code. CIDR refs are validated;
    /// only the Gateway central IpList store owns their compiled matcher.
    pub fn compile(&mut self, is_stream: bool) -> Result<(), String> {
        if let Some(e) = self.field_error(is_stream) {
            return Err(e);
        }
        match self.key {
            GeoRuleKey::Cidr | GeoRuleKey::Unsupported => {}
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
    pub fn matches(&self, ip: Option<IpAddr>, facts: &GeoDimFacts<'_>, mut cidr: impl FnMut(IpAddr) -> bool) -> bool {
        if self.cidr.invalid_input {
            return false;
        }
        // The CIDR callback belongs to an already validated owning runtime.
        // Geographic empty values still match nothing.
        match self.key {
            GeoRuleKey::Unsupported => false,
            GeoRuleKey::Cidr => ip.is_some_and(&mut cidr),
            GeoRuleKey::Country => Self::dimension_matches(&self.values, facts.country),
            GeoRuleKey::Continent => Self::dimension_matches(&self.values, facts.continent),
            GeoRuleKey::City => Self::dimension_matches(&self.values, facts.city),
            GeoRuleKey::Subdivision => Self::dimension_matches(&self.values, facts.subdivision),
        }
    }
}

impl GeoRuleGroup {
    pub fn check_fields(&self, is_stream: bool) -> Option<String> {
        if self.invalid_input {
            return Some("GeoIpLocation group contains unsupported fields".into());
        }
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
    pub fn matches(&self, ctx: &GeoRuleContext<'_>, mut cidr: impl FnMut(usize, IpAddr) -> bool) -> bool {
        if self.invalid_input {
            return false;
        }
        self.rules.iter().enumerate().all(|(index, rule)| {
            let (ip, facts) = match rule.source {
                GeoRuleSource::DirectPeerIp => (ctx.direct_peer_ip, &ctx.direct_peer),
                GeoRuleSource::RemoteIp => (ctx.remote_ip, &ctx.remote),
                GeoRuleSource::Unsupported => return false,
            };
            rule.matches(ip, facts, |ip| cidr(index, ip))
        })
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/geo_rule_test.rs"]
mod tests;
