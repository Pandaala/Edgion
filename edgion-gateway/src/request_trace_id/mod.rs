//! Canonical request identity and bounded, incremental correlation header state.
//!
//! W3C propagation is always initialized independently of RequestId plugin rules.
//! Header reconciliation is deliberately idempotent and reads the current state.
mod w3c;

use edgion_resources::{
    DefaultIdConfig, KeyGet, PluginValidation, RequestIdConfig, RequestIdGenerator, RequestIdGeneratorKind,
    RequestIdRule,
};
use http::{HeaderMap, HeaderName, HeaderValue};
use md5::{Digest, Md5};
use rand::Rng;
use serde::{ser::SerializeMap, Serialize, Serializer};

const DEFAULT_HEADER: &str = "x-trace-id";
use edgion_resources::{
    valid_request_id_value, REQUEST_ID_MAX_COUNT as MAX_IDS, REQUEST_ID_MAX_TOTAL_VALUE_LEN as MAX_TOTAL,
};

/// A rule compiled once when a resource generation is published.
#[derive(Clone, Debug)]
pub struct CompiledRequestIdRule {
    pub header: HeaderName,
    pub source: Option<KeyGet>,
    generate_when_missing: bool,
    generator: RequestIdGenerator,
    forward_upstream: bool,
    echo_response: bool,
}

impl CompiledRequestIdRule {
    pub fn new(rule: &RequestIdRule) -> Result<Self, String> {
        if let Some(error) = (RequestIdConfig {
            default_id: None,
            ids: vec![rule.clone()],
        })
        .validation_error()
        {
            return Err(error);
        }
        let header =
            HeaderName::from_bytes(rule.header.as_bytes()).map_err(|_| "invalid RequestId header".to_owned())?;
        Ok(Self {
            header,
            source: rule.source.clone(),
            generate_when_missing: rule.generate_when_missing,
            generator: rule.generator.clone(),
            forward_upstream: rule.forward_upstream,
            echo_response: rule.echo_response,
        })
    }
}

#[derive(Clone)]
struct W3cTraceContext {
    traceparent: HeaderValue,
    trace_id: [u8; 16],
    tracestate: Option<HeaderValue>,
}

impl std::fmt::Debug for W3cTraceContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("W3cTraceContext")
            .field("trace_id", &self.trace_id)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
struct CorrelationId {
    key: HeaderName,
    // None is a suppression marker. Keeping it here bounds tombstones as well.
    value: Option<HeaderValue>,
    forward_upstream: bool,
    echo_response: bool,
    sensitive: bool,
}

impl std::fmt::Debug for CorrelationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CorrelationId")
            .field("key", &self.key)
            .field("active", &self.value.is_some())
            .finish_non_exhaustive()
    }
}

/// Per-request identity. Default is explicitly uninitialized until parsed headers exist.
#[derive(Clone, Debug, Default)]
pub struct RequestTraceId {
    w3c: Option<W3cTraceContext>,
    correlation_ids: Vec<CorrelationId>,
    default_forward: bool,
    default_echo: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    Established,
    Preserved,
    Absent,
    LimitExceeded,
}

impl RequestTraceId {
    pub fn is_initialized(&self) -> bool {
        self.w3c.is_some()
    }
    pub fn is_empty(&self) -> bool {
        self.w3c.is_none()
    }

    /// Capture the request identity exactly once, before any application rejection.
    pub fn initialize(&mut self, headers: &HeaderMap) {
        if self.w3c.is_some() {
            return;
        }
        let inbound = single_header(headers, "traceparent")
            .filter(|value| value.to_str().ok().and_then(w3c::parse_trace_id).is_some());
        let (traceparent, tracestate) = if let Some(value) = inbound {
            (value, validated_tracestate(headers))
        } else {
            let root = w3c::generate_root(false);
            // Generated ASCII has a fixed valid header representation.
            (
                HeaderValue::try_from(root.into_traceparent()).expect("generated W3C header is valid"),
                None,
            )
        };
        let mut trace_id = [0; 16];
        for (byte, pair) in trace_id.iter_mut().zip(traceparent.as_bytes()[3..35].chunks_exact(2)) {
            *byte = hex_digit(pair[0]) * 16 + hex_digit(pair[1]);
        }
        self.w3c = Some(W3cTraceContext {
            traceparent,
            trace_id,
            tracestate,
        });
        self.default_forward = true;
        self.default_echo = true;
        let value = HeaderValue::from_str(self.standard_trace_id().unwrap_or_default()).expect("validated trace ID");
        self.correlation_ids.push(CorrelationId {
            key: HeaderName::from_static(DEFAULT_HEADER),
            value: Some(value),
            forward_upstream: true,
            echo_response: true,
            sensitive: false,
        });
    }

    pub fn standard_trace_id(&self) -> Option<&str> {
        self.w3c
            .as_ref()
            .and_then(|w| w.traceparent.to_str().ok())
            .map(|value| &value[3..35])
    }

    pub fn typed_trace_id(&self) -> Option<[u8; 16]> {
        self.w3c.as_ref().map(|w| w.trace_id)
    }

    pub fn correlation_id(&self, name: &str) -> Option<&str> {
        self.correlation_ids
            .iter()
            .find(|id| id.key.as_str().eq_ignore_ascii_case(name))
            .and_then(|id| id.value.as_ref())
            .and_then(|value| value.to_str().ok())
    }

    /// Patch default header emission; the canonical log identity is always retained.
    pub fn apply_default_patch(&mut self, patch: &DefaultIdConfig) {
        if self.w3c.is_none() {
            return;
        }
        if let Some(value) = patch.forward_upstream {
            self.default_forward = value;
        }
        if let Some(value) = patch.echo_response {
            self.default_echo = value;
        }
        let Some(index) = self.correlation_ids.iter().position(|id| id.key == DEFAULT_HEADER) else {
            return;
        };
        self.correlation_ids[index].forward_upstream = self.default_forward;
        self.correlation_ids[index].echo_response = self.default_echo;
        match patch.enabled {
            Some(false) => {
                self.correlation_ids[index].value = None;
            }
            Some(true) if self.correlation_ids[index].value.is_none() => {
                self.correlation_ids[index].value =
                    self.standard_trace_id().and_then(|id| HeaderValue::from_str(id).ok());
                self.correlation_ids[index].sensitive = false;
            }
            _ => {}
        }
    }

    /// The adapter supplies exactly one target header and extracts source through KeyGet.
    /// A suppressed target must not be resurrected from a stale inbound snapshot.
    pub fn apply_rule(
        &mut self,
        rule: &CompiledRequestIdRule,
        target: Option<HeaderValue>,
        source: Option<HeaderValue>,
    ) -> ApplyOutcome {
        let index = self.correlation_ids.iter().position(|id| id.key == rule.header);
        if index.is_some_and(|index| self.correlation_ids[index].value.is_some()) {
            return ApplyOutcome::Preserved;
        }
        if index.is_none() && self.correlation_ids.len() >= MAX_IDS {
            return ApplyOutcome::LimitExceeded;
        }
        let incoming = if index.is_none() {
            target.filter(valid_value)
        } else {
            None
        };
        let inbound_present = incoming.is_some();
        let copied = incoming.or_else(|| source.filter(valid_value));
        let sensitive = copied.is_some() && (inbound_present || !matches!(rule.source, Some(KeyGet::TraceId)));
        let value = copied.or_else(|| rule.generate_when_missing.then(|| generate(&rule.generator)).flatten());
        if let Some(value) = value.as_ref() {
            let current_bytes: usize = self
                .correlation_ids
                .iter()
                .filter_map(|id| id.value.as_ref())
                .map(|v| v.as_bytes().len())
                .sum();
            if current_bytes + value.as_bytes().len() > MAX_TOTAL {
                return ApplyOutcome::LimitExceeded;
            }
        }
        let outcome = if value.is_some() {
            ApplyOutcome::Established
        } else {
            ApplyOutcome::Absent
        };
        let id = CorrelationId {
            key: rule.header.clone(),
            value,
            forward_upstream: rule.forward_upstream,
            echo_response: rule.echo_response,
            sensitive,
        };
        if let Some(index) = index {
            self.correlation_ids[index] = id;
        } else {
            self.correlation_ids.push(id);
        }
        outcome
    }

    pub fn reconcile_request(&self, headers: &mut impl ManagedHeaders) {
        if let Some(w3c) = &self.w3c {
            headers.set_managed(HeaderName::from_static("traceparent"), w3c.traceparent.clone());
            if let Some(value) = &w3c.tracestate {
                headers.set_managed(HeaderName::from_static("tracestate"), value.clone());
            } else {
                headers.remove_managed(&HeaderName::from_static("tracestate"));
            }
        }
        for id in &self.correlation_ids {
            if let Some(value) = id.value.as_ref().filter(|_| id.forward_upstream) {
                headers.set_managed(id.key.clone(), value.clone());
            } else {
                headers.remove_managed(&id.key);
            }
        }
    }

    pub fn reconcile_response(&self, headers: &mut impl ManagedHeaders) {
        for id in &self.correlation_ids {
            if let Some(value) = id.value.as_ref().filter(|_| id.echo_response) {
                headers.set_managed(id.key.clone(), value.clone());
            } else {
                headers.remove_managed(&id.key);
            }
        }
    }
}

/// Header mutations preserve Pingora's HTTP/1 case map as well as its value map.
pub trait ManagedHeaders {
    fn set_managed(&mut self, name: HeaderName, value: HeaderValue);
    fn remove_managed(&mut self, name: &HeaderName);
}

impl ManagedHeaders for HeaderMap {
    fn set_managed(&mut self, name: HeaderName, value: HeaderValue) {
        self.insert(name, value);
    }
    fn remove_managed(&mut self, name: &HeaderName) {
        self.remove(name);
    }
}

macro_rules! pingora_managed_headers {
    ($header:ty) => {
        impl ManagedHeaders for $header {
            fn set_managed(&mut self, name: HeaderName, value: HeaderValue) {
                // Both typed inputs are prevalidated; Pingora only fails conversion here.
                let _ = self.insert_header(name, value);
            }
            fn remove_managed(&mut self, name: &HeaderName) {
                self.remove_header(name);
            }
        }
    };
}
pingora_managed_headers!(pingora_http::RequestHeader);
pingora_managed_headers!(pingora_http::ResponseHeader);

/// Duplicate header lines are ambiguous even when their values happen to match.
pub fn single_header(headers: &HeaderMap, name: &str) -> Option<HeaderValue> {
    let mut values = headers.get_all(name).iter();
    let first = values.next()?;
    if values.next().is_some() || first.is_sensitive() {
        return None;
    }
    Some(first.clone())
}

fn valid_value(value: &HeaderValue) -> bool {
    !value.is_sensitive() && valid_request_id_value(value.as_bytes())
}

fn hex_digit(byte: u8) -> u8 {
    if byte <= b'9' {
        byte - b'0'
    } else {
        byte - b'a' + 10
    }
}

fn generate(config: &RequestIdGenerator) -> Option<HeaderValue> {
    let mut rng = rand::rng();
    let suffix = match config.kind {
        RequestIdGeneratorKind::UuidV4 => uuid::Uuid::new_v4().to_string(),
        RequestIdGeneratorKind::UuidV7 => uuid::Uuid::now_v7().to_string(),
        RequestIdGeneratorKind::RandomHex => {
            let count = config.bytes.unwrap_or(16);
            if !(8..=64).contains(&count) {
                return None;
            }
            let bytes: Vec<u8> = (0..count).map(|_| rng.random()).collect();
            lower_hex(&bytes)
        }
        RequestIdGeneratorKind::NanoId => {
            const ALPHABET: &[u8; 64] = b"_-0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
            let length = config.length.unwrap_or(21);
            if !(8..=128).contains(&length) {
                return None;
            }
            (0..length).map(|_| ALPHABET[rng.random_range(0..64)] as char).collect()
        }
        RequestIdGeneratorKind::Md5 => {
            let seed: [u8; 16] = rng.random();
            lower_hex(&Md5::digest(seed))
        }
    };
    let value = HeaderValue::from_str(&format!("{}{}", config.prefix, suffix)).ok()?;
    valid_value(&value).then_some(value)
}

fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 15) as usize] as char);
    }
    output
}

fn validated_tracestate(headers: &HeaderMap) -> Option<HeaderValue> {
    let mut combined = String::new();
    let mut keys = Vec::new();
    for line in headers.get_all("tracestate") {
        if line.is_sensitive() {
            return None;
        }
        let line = line.to_str().ok()?;
        if combined
            .len()
            .saturating_add(line.len())
            .saturating_add(usize::from(!combined.is_empty()))
            > 512
        {
            return None;
        }
        if !combined.is_empty() {
            combined.push(',');
        }
        combined.push_str(line);
    }
    if combined.is_empty() {
        return None;
    }
    for member in combined.split(',') {
        let member = member.trim_matches([' ', '\t']);
        // W3C explicitly permits empty list members, including OWS-only ones.
        if member.is_empty() {
            continue;
        }
        let (key, value) = member.split_once('=')?;
        if keys.len() == 32
            || keys.contains(&key)
            || !valid_state_key(key)
            || value.is_empty()
            || value.len() > 256
            || value.ends_with(' ')
            || !value
                .bytes()
                .all(|b| (0x20..=0x7e).contains(&b) && b != b',' && b != b'=')
        {
            return None;
        }
        keys.push(key);
    }
    HeaderValue::from_str(&combined).ok()
}

fn valid_state_key(key: &str) -> bool {
    fn tail(value: &str) -> bool {
        value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-*/".contains(&b))
    }
    if key.is_empty() || key.len() > 256 {
        return false;
    }
    if let Some((tenant, system)) = key.split_once('@') {
        !tenant.is_empty()
            && tenant.len() <= 241
            && tenant.as_bytes()[0].is_ascii_alphanumeric()
            && tail(tenant)
            && !system.is_empty()
            && system.len() <= 14
            && system.as_bytes()[0].is_ascii_lowercase()
            && tail(system)
    } else {
        key.as_bytes()[0].is_ascii_lowercase() && tail(key)
    }
}

impl Serialize for RequestTraceId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        if let Some(id) = self.standard_trace_id() {
            map.serialize_entry(DEFAULT_HEADER, id)?;
        }
        for id in &self.correlation_ids {
            // The reserved key is emitted from W3C state even when its wire header is disabled.
            if id.key == DEFAULT_HEADER {
                continue;
            }
            if let Some(value) = id.value.as_ref().and_then(|value| value.to_str().ok()) {
                if id.sensitive {
                    edgion_resources::sensitive::apply_sensitive(value, |masked| {
                        map.serialize_entry(id.key.as_str(), masked)
                    })?;
                } else {
                    map.serialize_entry(id.key.as_str(), value)?;
                }
            }
        }
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const PARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    fn rule(json: serde_json::Value) -> CompiledRequestIdRule {
        CompiledRequestIdRule::new(&serde_json::from_value(json).unwrap()).unwrap()
    }
    fn state() -> RequestTraceId {
        let mut state = RequestTraceId::default();
        let mut headers = HeaderMap::new();
        headers.insert("traceparent", HeaderValue::from_static(PARENT));
        state.initialize(&headers);
        state
    }

    #[test]
    fn initializes_once_and_replaces_conflicting_default_with_w3c() {
        let mut state = RequestTraceId::default();
        assert!(state.is_empty());
        assert_eq!(serde_json::to_value(&state).unwrap(), serde_json::json!({}));
        let mut headers = HeaderMap::new();
        headers.insert("traceparent", HeaderValue::from_static(PARENT));
        headers.insert("x-trace-id", HeaderValue::from_static("legacy-123"));
        state.initialize(&headers);
        state.initialize(&HeaderMap::new());
        assert_eq!(state.correlation_id("X-Trace-Id"), state.standard_trace_id());
        assert_eq!(state.standard_trace_id(), Some("4bf92f3577b34da6a3ce929d0e0e4736"));
        assert_eq!(
            state.typed_trace_id().map(|id| lower_hex(&id)).as_deref(),
            state.standard_trace_id()
        );
        state.reconcile_request(&mut headers);
        assert_eq!(headers["traceparent"], PARENT);
        assert_eq!(headers[DEFAULT_HEADER], state.standard_trace_id().unwrap());
        assert_eq!(
            serde_json::to_value(&state).unwrap()[DEFAULT_HEADER],
            state.standard_trace_id().unwrap()
        );
    }

    #[test]
    fn three_gateway_hops_preserve_w3c_and_first_custom_id() {
        for incoming_default in [None, Some("legacy-business-id")] {
            let mut wire = HeaderMap::new();
            wire.insert("traceparent", HeaderValue::from_static(PARENT));
            wire.insert("tracestate", HeaderValue::from_static("vendor=state"));
            if let Some(value) = incoming_default {
                wire.insert(DEFAULT_HEADER, HeaderValue::from_static(value));
            }
            let mut first_custom = None;
            for prefix in ["first-", "second-", "third-"] {
                let mut gateway = RequestTraceId::default();
                gateway.initialize(&wire);
                let custom = rule(serde_json::json!({
                    "header": "x-order-id",
                    "generator": {"type": "md5", "prefix": prefix}
                }));
                gateway.apply_rule(&custom, single_header(&wire, "x-order-id"), None);
                gateway.reconcile_request(&mut wire);
                let expected_custom = first_custom.get_or_insert_with(|| wire["x-order-id"].clone());
                assert!(expected_custom.to_str().unwrap().starts_with("first-"));
                assert_eq!(&wire["x-order-id"], expected_custom);
                assert_eq!(wire[DEFAULT_HEADER], "4bf92f3577b34da6a3ce929d0e0e4736");
                assert_eq!(wire["traceparent"], PARENT);
                assert_eq!(wire["tracestate"], "vendor=state");

                let mut response = HeaderMap::new();
                response.insert("x-order-id", HeaderValue::from_static("backend-stale"));
                gateway.reconcile_response(&mut response);
                assert_eq!(response["x-order-id"], wire["x-order-id"]);
                assert_eq!(response[DEFAULT_HEADER], wire[DEFAULT_HEADER]);
                assert!(!response.contains_key("traceparent"));
            }
        }
    }

    #[test]
    fn invalid_or_duplicate_parent_restarts_and_drops_state() {
        for values in [vec!["bad"], vec![PARENT, PARENT]] {
            let mut headers = HeaderMap::new();
            for value in values {
                headers.append("traceparent", HeaderValue::from_str(value).unwrap());
            }
            headers.insert("tracestate", HeaderValue::from_static("vendor=value"));
            let mut state = RequestTraceId::default();
            state.initialize(&headers);
            state.reconcile_request(&mut headers);
            assert_ne!(headers["traceparent"], PARENT);
            assert!(w3c::parse_trace_id(headers["traceparent"].to_str().unwrap()).is_some());
            assert!(headers["traceparent"].to_str().unwrap().ends_with("-00"));
            assert!(!headers.contains_key("tracestate"));
        }
    }

    #[test]
    fn disabled_default_suppresses_wire_headers_but_retains_log_identity() {
        let mut state = state();
        let patch: DefaultIdConfig = serde_json::from_value(serde_json::json!({"enabled":false})).unwrap();
        state.apply_default_patch(&patch);
        let mut headers = HeaderMap::new();
        headers.insert("x-trace-id", HeaderValue::from_static("stale"));
        state.reconcile_response(&mut headers);
        assert!(!headers.contains_key("x-trace-id"));
        state.reconcile_request(&mut headers);
        assert!(!headers.contains_key(DEFAULT_HEADER));
        assert_eq!(headers["traceparent"], PARENT);
        assert_eq!(
            serde_json::to_value(&state).unwrap(),
            serde_json::json!({
                "x-trace-id": "4bf92f3577b34da6a3ce929d0e0e4736"
            })
        );
        state.apply_default_patch(&patch);
        state.reconcile_response(&mut headers);
        assert!(!headers.contains_key(DEFAULT_HEADER));
        assert!(state.standard_trace_id().is_some());
    }

    #[test]
    fn first_write_preserves_value_and_policy() {
        let mut state = state();
        let first = rule(serde_json::json!({"header":"x-order-id","echoResponse":false,"forwardUpstream":false}));
        let later = rule(serde_json::json!({"header":"X-Order-ID"}));
        state.apply_rule(&first, Some(HeaderValue::from_static("one")), None);
        assert_eq!(
            state.apply_rule(&later, Some(HeaderValue::from_static("two")), None),
            ApplyOutcome::Preserved
        );
        let mut headers = HeaderMap::new();
        headers.insert("x-order-id", HeaderValue::from_static("backend"));
        state.reconcile_response(&mut headers);
        assert!(!headers.contains_key("x-order-id"));
        state.reconcile_request(&mut headers);
        assert!(!headers.contains_key("x-order-id"));
        assert_eq!(state.correlation_id("x-order-id"), Some("one"));
    }

    #[test]
    fn prefixes_apply_only_to_generated_values() {
        let rule = rule(serde_json::json!({"header":"x-order-id","generator":{"type":"md5","prefix":"order-"}}));
        let mut first = state();
        first.apply_rule(&rule, Some(HeaderValue::from_static("client")), None);
        assert_eq!(first.correlation_id("x-order-id"), Some("client"));
        let mut second = state();
        second.apply_rule(&rule, None, Some(HeaderValue::from_static("copied")));
        assert_eq!(second.correlation_id("x-order-id"), Some("copied"));
        let mut third = state();
        third.apply_rule(&rule, None, None);
        let generated = third.correlation_id("x-order-id").unwrap();
        assert!(generated.starts_with("order-"));
        assert_eq!(generated.len(), 38);
    }

    #[test]
    fn rejects_ambiguous_sensitive_and_oversized_values() {
        let rule = rule(serde_json::json!({"header":"x-order-id","generateWhenMissing":false}));
        for value in ["", "a,b", "with space", &"a".repeat(257)] {
            let mut state = state();
            assert_eq!(
                state.apply_rule(&rule, Some(HeaderValue::from_str(value).unwrap()), None),
                ApplyOutcome::Absent
            );
        }
        let mut sensitive = HeaderValue::from_static("credential");
        sensitive.set_sensitive(true);
        let mut state = state();
        assert_eq!(state.apply_rule(&rule, None, Some(sensitive)), ApplyOutcome::Absent);
        let mut headers = HeaderMap::new();
        headers.append("x-order-id", HeaderValue::from_static("one"));
        headers.append("x-order-id", HeaderValue::from_static("one"));
        assert!(single_header(&headers, "x-order-id").is_none());
    }

    #[test]
    fn composition_cap_also_bounds_absent_managed_headers() {
        let mut state = state();
        for index in 0..15 {
            let rule = rule(serde_json::json!({"header":format!("x-id-{index}"),"generateWhenMissing":false}));
            assert_eq!(state.apply_rule(&rule, None, None), ApplyOutcome::Absent);
        }
        let rule = rule(serde_json::json!({"header":"x-overflow"}));
        assert_eq!(state.apply_rule(&rule, None, None), ApplyOutcome::LimitExceeded);
        assert_eq!(state.correlation_ids.len(), 16);
    }

    #[test]
    fn tracestate_grammar_and_combined_limits() {
        let mut headers = HeaderMap::new();
        headers.append("tracestate", HeaderValue::from_static("vendor=one"));
        headers.append("tracestate", HeaderValue::from_static("tenant@system=two"));
        assert_eq!(validated_tracestate(&headers).unwrap(), "vendor=one,tenant@system=two");
        for value in [
            "vendor=one,vendor=two",
            "A=bad",
            "vendor=bad=value",
            "vendor=",
            "vendor=value ",
        ] {
            headers.insert("tracestate", HeaderValue::from_str(value).unwrap());
            // A trailing OWS is permitted on the list member by the W3C grammar.
            if value.ends_with(' ') {
                assert!(validated_tracestate(&headers).is_some());
            } else {
                assert!(validated_tracestate(&headers).is_none(), "{value}");
            }
        }
        headers.insert(
            "tracestate",
            HeaderValue::from_str(&format!("vendor={}", "a".repeat(257))).unwrap(),
        );
        assert!(validated_tracestate(&headers).is_none());
    }

    #[test]
    fn tracestate_accepts_empty_members_and_whitespace_header_lines() {
        for value in ["vendor=value,", "vendor=one,,other=two", ", \t,vendor=one, "] {
            let mut headers = HeaderMap::new();
            headers.insert("tracestate", HeaderValue::from_str(value).unwrap());
            assert_eq!(validated_tracestate(&headers).unwrap(), value);
            headers.append("tracestate", HeaderValue::from_static(" \t"));
            assert!(validated_tracestate(&headers).is_some());
        }
        let mut headers = HeaderMap::new();
        headers.insert("tracestate", HeaderValue::from_static(""));
        let mut state = RequestTraceId::default();
        state.initialize(&headers);
        assert!(state.typed_trace_id().is_some());
    }

    #[test]
    fn serialization_uses_named_map_and_excludes_wire_context() {
        let mut state = state();
        let custom = rule(serde_json::json!({"header":"x-order-id","source":{"type":"traceId"}}));
        state.apply_rule(
            &custom,
            None,
            Some(HeaderValue::from_static("4bf92f3577b34da6a3ce929d0e0e4736")),
        );
        let serialized = serde_json::to_value(&state).unwrap();
        assert_eq!(
            serialized,
            serde_json::json!({
                "x-trace-id": "4bf92f3577b34da6a3ce929d0e0e4736",
                "x-order-id": "4bf92f3577b34da6a3ce929d0e0e4736"
            })
        );
        assert_eq!(
            serde_json::to_string(&state).unwrap().matches("\"x-trace-id\"").count(),
            1
        );
    }

    #[test]
    fn compiled_rule_rejects_reserved_default_name() {
        for header in ["x-trace-id", "X-Trace-ID"] {
            let config: RequestIdRule = serde_json::from_value(serde_json::json!({"header":header})).unwrap();
            assert!(CompiledRequestIdRule::new(&config).is_err());
        }
    }

    #[test]
    fn default_patches_are_incremental_and_can_reenable() {
        let mut state = state();
        state.apply_default_patch(&DefaultIdConfig {
            enabled: Some(false),
            echo_response: Some(false),
            ..Default::default()
        });
        state.apply_default_patch(&DefaultIdConfig::default());
        assert!(state.correlation_id(DEFAULT_HEADER).is_none());
        state.apply_default_patch(&DefaultIdConfig {
            enabled: Some(true),
            ..Default::default()
        });
        assert_eq!(state.correlation_id(DEFAULT_HEADER), state.standard_trace_id());
        let mut headers = HeaderMap::new();
        state.reconcile_response(&mut headers);
        assert!(!headers.contains_key(DEFAULT_HEADER));
        state.reconcile_request(&mut headers);
        assert!(headers.contains_key(DEFAULT_HEADER));
    }

    #[test]
    fn future_version_wire_representation_survives_reconciliation() {
        let future = "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-future";
        let mut headers = HeaderMap::new();
        headers.insert("traceparent", HeaderValue::from_static(future));
        let mut state = RequestTraceId::default();
        state.initialize(&headers);
        headers.insert("traceparent", HeaderValue::from_static("modified"));
        state.reconcile_request(&mut headers);
        assert_eq!(headers["traceparent"], future);
    }

    #[test]
    fn all_generators_have_expected_shapes() {
        for (kind, length) in [
            ("uuidV4", 36),
            ("uuidV7", 36),
            ("randomHex", 32),
            ("nanoId", 21),
            ("md5", 32),
        ] {
            let config: RequestIdGenerator = serde_json::from_value(serde_json::json!({"type":kind})).unwrap();
            let one = generate(&config).unwrap();
            let two = generate(&config).unwrap();
            assert_eq!(one.as_bytes().len(), length);
            assert_ne!(one, two);
        }
    }
}
