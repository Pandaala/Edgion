//! Typed mTLS SAN allowlist entries for `EdgionTls.spec.clientAuth.allowedSans`.
//!
//! Operator-facing schema only. Handshake matching lives in `edgion-common`
//! so Controller validation and Gateway enforcement share one implementation.
//! `RegularExpression` is compiled with bounded `RegexBuilder` limits; the
//! compiled automaton is never serialized.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::sync::OnceLock;

use crate::gwapi_types::{HOSTNAME_MAX_LEN, HOSTNAME_PATTERN};

/// Maximum source length for a SAN `RegularExpression` value.
pub const ALLOWED_SAN_REGEX_MAX_LEN: usize = 1024;

/// Compiled regex program / DFA cap (1 MiB), matching DSL `Constant::Regex`.
pub const ALLOWED_SAN_REGEX_SIZE_LIMIT: usize = 1 << 20;

/// SAN kind to match, aligned with RFC 5280 GeneralName plus OtherName.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum AllowedSanType {
    #[serde(rename = "DNS")]
    Dns,
    #[serde(rename = "URI")]
    Uri,
    #[serde(rename = "Email")]
    Email,
    #[serde(rename = "IP")]
    Ip,
    OtherName,
}

/// How `value` is compared to a certificate SAN of the selected type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub enum AllowedSanMatch {
    #[default]
    Exact,
    Prefix,
    Suffix,
    Contains,
    RegularExpression,
}

/// One typed SAN allowlist entry.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AllowedSan {
    #[serde(rename = "type")]
    pub san_type: AllowedSanType,

    /// Comparison operator. Defaults to Exact.
    #[serde(rename = "match", default, skip_serializing_if = "is_default_match")]
    pub match_kind: AllowedSanMatch,

    pub value: String,

    #[serde(default, skip_serializing_if = "is_false")]
    pub ignore_case: bool,

    /// Required when `type` is `OtherName`; forbidden otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oid: Option<String>,

    /// Filled at Gateway ingest / test compile. Never part of CRD or conf_sync.
    #[serde(skip)]
    #[schemars(skip)]
    pub compiled_regex: Option<regex::Regex>,
}

fn is_default_match(kind: &AllowedSanMatch) -> bool {
    *kind == AllowedSanMatch::Exact
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl AllowedSan {
    pub fn exact(san_type: AllowedSanType, value: impl Into<String>) -> Self {
        Self {
            san_type,
            match_kind: AllowedSanMatch::Exact,
            value: value.into(),
            ignore_case: false,
            oid: None,
            compiled_regex: None,
        }
    }

    pub fn other_name(oid: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            san_type: AllowedSanType::OtherName,
            match_kind: AllowedSanMatch::Exact,
            value: value.into(),
            ignore_case: false,
            oid: Some(oid.into()),
            compiled_regex: None,
        }
    }

    /// Compile a `RegularExpression` entry. No-op for other match kinds.
    pub fn compile(&mut self) -> Result<(), String> {
        if self.match_kind == AllowedSanMatch::RegularExpression {
            self.compiled_regex = Some(compile_bounded_regex(self.value.trim())?);
        }
        Ok(())
    }
}

/// Compile an operator regex under the shared DFA caps.
pub fn compile_bounded_regex(pattern: &str) -> Result<regex::Regex, String> {
    if pattern.len() > ALLOWED_SAN_REGEX_MAX_LEN {
        return Err(format!(
            "RegularExpression value exceeds {ALLOWED_SAN_REGEX_MAX_LEN} bytes"
        ));
    }
    regex::RegexBuilder::new(pattern)
        .size_limit(ALLOWED_SAN_REGEX_SIZE_LIMIT)
        .dfa_size_limit(ALLOWED_SAN_REGEX_SIZE_LIMIT)
        .build()
        .map_err(|_| {
            format!("RegularExpression is invalid or exceeds the {ALLOWED_SAN_REGEX_SIZE_LIMIT}-byte compiled limit")
        })
}

/// Schema checks that do not compile a regex. Safe on the handshake path.
pub fn allowed_san_schema_error(entry: &AllowedSan) -> Option<String> {
    let value = entry.value.trim();
    if value.is_empty() {
        return Some("value must be a non-empty string".to_string());
    }

    match entry.san_type {
        AllowedSanType::OtherName => {
            let Some(oid) = entry.oid.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
                return Some("oid is required when type is OtherName".to_string());
            };
            if !is_dotted_oid(oid) {
                return Some("oid must be a dotted-decimal object identifier".to_string());
            }
        }
        _ => {
            if entry.oid.is_some() {
                return Some("oid is only valid when type is OtherName".to_string());
            }
        }
    }

    match (entry.san_type, entry.match_kind) {
        (AllowedSanType::Dns, AllowedSanMatch::Exact) => {
            if value.len() > HOSTNAME_MAX_LEN as usize || !hostname_pattern().is_match(value) {
                return Some("DNS Exact value must be a Gateway API hostname (optional leftmost *.)".to_string());
            }
        }
        (AllowedSanType::Ip, AllowedSanMatch::Exact) if value.parse::<IpAddr>().is_err() => {
            return Some("IP Exact value must be a parseable IP address".to_string());
        }
        _ => {}
    }

    None
}

/// Admission / ingest error for one allowlist entry, including regex compile.
pub fn allowed_san_config_error(entry: &AllowedSan) -> Option<String> {
    if let Some(err) = allowed_san_schema_error(entry) {
        return Some(err);
    }
    if entry.match_kind == AllowedSanMatch::RegularExpression {
        if let Err(err) = compile_bounded_regex(entry.value.trim()) {
            return Some(err);
        }
    }
    None
}

fn hostname_pattern() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(HOSTNAME_PATTERN).expect("HOSTNAME_PATTERN must compile"))
}

fn is_dotted_oid(oid: &str) -> bool {
    let mut arcs = 0usize;
    for arc in oid.split('.') {
        if arc.is_empty() || !arc.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        if arc.len() > 1 && arc.starts_with('0') {
            return false;
        }
        arcs += 1;
    }
    arcs >= 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_compact_dns_exact() {
        let entry: AllowedSan = serde_yaml::from_str("type: DNS\nvalue: client1.example.com\n").unwrap();
        assert_eq!(entry.san_type, AllowedSanType::Dns);
        assert_eq!(entry.match_kind, AllowedSanMatch::Exact);
        assert!(!entry.ignore_case);
        assert!(entry.oid.is_none());
        assert!(allowed_san_config_error(&entry).is_none());
    }

    #[test]
    fn rejects_unknown_field() {
        let err = serde_yaml::from_str::<AllowedSan>("type: DNS\nvalue: a.example.com\nextra: 1\n").unwrap_err();
        assert!(err.to_string().contains("unknown field"));
    }

    #[test]
    fn other_name_requires_oid() {
        let entry = AllowedSan::exact(AllowedSanType::OtherName, "alice@corp.example.com");
        assert!(allowed_san_config_error(&entry).unwrap().contains("oid is required"));
    }

    #[test]
    fn oid_forbidden_on_dns() {
        let mut entry = AllowedSan::exact(AllowedSanType::Dns, "client.example.com");
        entry.oid = Some("1.2.3".to_string());
        assert!(allowed_san_config_error(&entry).unwrap().contains("only valid"));

        let empty_oid: AllowedSan = serde_yaml::from_str("type: DNS\nvalue: client.example.com\noid: \"\"\n").unwrap();
        assert!(allowed_san_config_error(&empty_oid).unwrap().contains("only valid"));
    }

    #[test]
    fn dns_exact_rejects_uppercase_and_nested_wildcard() {
        let upper = AllowedSan::exact(AllowedSanType::Dns, "Client.example.com");
        assert!(allowed_san_config_error(&upper).is_some());
        let nested = AllowedSan::exact(AllowedSanType::Dns, "*.*.example.com");
        assert!(allowed_san_config_error(&nested).is_some());
        let ok = AllowedSan::exact(AllowedSanType::Dns, "*.internal.example.com");
        assert!(allowed_san_config_error(&ok).is_none());
    }

    #[test]
    fn ip_exact_rejects_garbage() {
        let entry = AllowedSan::exact(AllowedSanType::Ip, "not-an-ip");
        assert!(allowed_san_config_error(&entry).unwrap().contains("parseable"));
    }

    #[test]
    fn regex_compile_and_size_cap() {
        let mut entry = AllowedSan::exact(AllowedSanType::Uri, "^spiffe://cluster\\.local/.*$");
        entry.match_kind = AllowedSanMatch::RegularExpression;
        assert!(allowed_san_config_error(&entry).is_none());
        entry.compile().unwrap();
        assert!(entry.compiled_regex.is_some());

        let too_long = AllowedSan {
            san_type: AllowedSanType::Dns,
            match_kind: AllowedSanMatch::RegularExpression,
            value: "a".repeat(ALLOWED_SAN_REGEX_MAX_LEN + 1),
            ignore_case: false,
            oid: None,
            compiled_regex: None,
        };
        assert!(allowed_san_config_error(&too_long).unwrap().contains("exceeds"));
    }

    #[test]
    fn empty_value_rejected() {
        let entry = AllowedSan::exact(AllowedSanType::Uri, "  ");
        assert!(allowed_san_config_error(&entry).unwrap().contains("non-empty"));
    }
}
