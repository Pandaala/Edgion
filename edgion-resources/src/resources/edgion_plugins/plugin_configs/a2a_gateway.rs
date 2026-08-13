//! A2A Gateway Phase 1 configuration and Agent Card validation.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Write};
use url::Url;

use crate::resources::edgion_plugins::PluginValidation;

use super::json_schema_validation::profile_json_value;

/// Inclusive compact-JSON limit for the operator-owned Agent Card.
pub const A2A_AGENT_CARD_MAX_BYTES: usize = 1024 * 1024;

/// Request-stage A2A gateway configuration.
///
/// The card remains a JSON value so unknown A2A fields survive publication and
/// rewrite. The enclosing plugin configuration is intentionally strict.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct A2aGatewayConfig {
    /// Operator-owned A2A v1 Agent Card. Validation enforces the protocol's
    /// required identity, capability, interface, mode, and skill fields while
    /// preserving extension fields for publication.
    pub card: Value,

    /// Optional trusted public origin used instead of the routed request origin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_url: Option<String>,

    /// Forward requests that fail the A2A protocol/version gate.
    ///
    /// This does not change canonical request-body read failures.
    #[serde(default)]
    pub fail_open: bool,
}

impl A2aGatewayConfig {
    /// Validate the static card and return its compact JSON representation.
    ///
    /// Gateway construction can retain these exact bytes as its immutable input
    /// snapshot without reimplementing admission checks.
    pub fn validated_card_bytes(&self) -> Result<Vec<u8>, String> {
        profile_json_value(&self.card).map_err(|_| "card exceeds the JSON depth or node limit".to_string())?;
        let mut writer = LimitedJsonWriter::new(A2A_AGENT_CARD_MAX_BYTES);
        if serde_json::to_writer(&mut writer, &self.card).is_err() {
            return if writer.overflowed {
                Err(format!(
                    "card compact JSON must not exceed {A2A_AGENT_CARD_MAX_BYTES} bytes"
                ))
            } else {
                Err("card cannot be serialized as JSON".to_string())
            };
        }
        validate_card_shape(&self.card)?;
        Ok(writer.bytes)
    }

    /// Parse the optional external URL after enforcing origin-only semantics.
    pub fn validated_external_origin(&self) -> Result<Option<Url>, String> {
        self.external_url
            .as_deref()
            .map(|value| validate_http_url(value, true).map_err(|error| format!("externalUrl {error}")))
            .transpose()
    }
}

struct LimitedJsonWriter {
    bytes: Vec<u8>,
    limit: usize,
    overflowed: bool,
}

impl LimitedJsonWriter {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(64 * 1024)),
            limit,
            overflowed: false,
        }
    }
}

impl Write for LimitedJsonWriter {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        if self
            .bytes
            .len()
            .checked_add(input.len())
            .is_none_or(|length| length > self.limit)
        {
            self.overflowed = true;
            return Err(io::Error::other("compact JSON limit exceeded"));
        }
        self.bytes.extend_from_slice(input);
        Ok(input.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl PluginValidation for A2aGatewayConfig {
    fn validation_error(&self) -> Option<String> {
        self.validated_card_bytes()
            .err()
            .or_else(|| self.validated_external_origin().err())
    }
}

fn validate_card_shape(card: &Value) -> Result<(), String> {
    let object = card
        .as_object()
        .ok_or_else(|| "card must be a JSON object".to_string())?;

    for field in ["name", "description", "version"] {
        require_non_empty_string(object, field, "card")?;
    }
    if !object.get("capabilities").is_some_and(Value::is_object) {
        return Err("card.capabilities must be an object".to_string());
    }

    if object.contains_key("url") {
        return Err("card.url is the legacy A2A shape; use supportedInterfaces".to_string());
    }

    if let Some(signatures) = object.get("signatures") {
        match signatures.as_array() {
            Some(values) if values.is_empty() => {}
            _ => return Err("card.signatures must be absent or an empty array".to_string()),
        }
    }

    let interfaces = object
        .get("supportedInterfaces")
        .and_then(Value::as_array)
        .ok_or_else(|| "card.supportedInterfaces must be an array".to_string())?;
    if interfaces.is_empty() {
        return Err("card.supportedInterfaces must not be empty".to_string());
    }

    let mut supports_jsonrpc_v1 = false;
    for (index, interface) in interfaces.iter().enumerate() {
        let interface = interface
            .as_object()
            .ok_or_else(|| format!("card.supportedInterfaces[{index}] must be an object"))?;
        let path = format!("card.supportedInterfaces[{index}]");
        let raw_url = require_non_empty_string(interface, "url", &path)?;
        let protocol_binding = require_non_empty_string(interface, "protocolBinding", &path)?;
        let protocol_version = require_non_empty_string(interface, "protocolVersion", &path)?;

        if protocol_binding == "JSONRPC" && protocol_version == "1.0" {
            validate_http_url(raw_url, false)
                .map_err(|error| format!("card.supportedInterfaces[{index}].url {error}"))?;
            supports_jsonrpc_v1 = true;
        } else if raw_url.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
            return Err(format!(
                "card.supportedInterfaces[{index}].url must not contain whitespace/control characters"
            ));
        }
    }

    if !supports_jsonrpc_v1 {
        return Err("card.supportedInterfaces must contain a JSONRPC/1.0 interface".to_string());
    }
    validate_non_empty_string_array(object, "defaultInputModes", "card")?;
    validate_non_empty_string_array(object, "defaultOutputModes", "card")?;

    let skills = object
        .get("skills")
        .and_then(Value::as_array)
        .ok_or_else(|| "card.skills must be an array".to_string())?;
    if skills.is_empty() {
        return Err("card.skills must not be empty".to_string());
    }
    for (index, skill) in skills.iter().enumerate() {
        let skill = skill
            .as_object()
            .ok_or_else(|| format!("card.skills[{index}] must be an object"))?;
        let path = format!("card.skills[{index}]");
        for field in ["id", "name", "description"] {
            require_non_empty_string(skill, field, &path)?;
        }
        validate_non_empty_string_array(skill, "tags", &path)?;
    }
    Ok(())
}

fn require_non_empty_string<'a>(
    object: &'a serde_json::Map<String, Value>,
    field: &str,
    path: &str,
) -> Result<&'a str, String> {
    let value = object
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{path}.{field} must be a non-empty string"))?;
    if value.trim().is_empty() {
        return Err(format!("{path}.{field} must be a non-empty string"));
    }
    Ok(value)
}

fn validate_non_empty_string_array(
    object: &serde_json::Map<String, Value>,
    field: &str,
    path: &str,
) -> Result<(), String> {
    let values = object
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{path}.{field} must be an array"))?;
    if values.is_empty() {
        return Err(format!("{path}.{field} must not be empty"));
    }
    for (index, value) in values.iter().enumerate() {
        if value.as_str().is_none_or(|value| value.trim().is_empty()) {
            return Err(format!("{path}.{field}[{index}] must be a non-empty string"));
        }
    }
    Ok(())
}

fn validate_http_url(value: &str, origin_only: bool) -> Result<Url, String> {
    if value.is_empty() || value.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
        return Err("must not be empty or contain whitespace/control characters".to_string());
    }
    let parsed = Url::parse(value).map_err(|_| "must be an absolute HTTP(S) URL".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host().is_none() {
        return Err("must be an absolute HTTP(S) URL".to_string());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("must not contain user information".to_string());
    }
    if parsed.fragment().is_some() {
        return Err("must not contain a fragment".to_string());
    }
    if origin_only && (parsed.path() != "/" || parsed.query().is_some()) {
        return Err("must contain only an origin, without a path or query".to_string());
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_card() -> Value {
        json!({
            "name": "example",
            "description": "Example agent",
            "version": "1.0.0",
            "capabilities": {},
            "supportedInterfaces": [
                {
                    "url": "https://private.example/rpc?tenant=blue",
                    "protocolBinding": "JSONRPC",
                    "protocolVersion": "1.0",
                    "futureField": { "preserved": true }
                }
            ],
            "defaultInputModes": ["application/json"],
            "defaultOutputModes": ["application/json"],
            "skills": [{
                "id": "echo",
                "name": "Echo",
                "description": "Echo a message",
                "tags": ["example"]
            }],
            "futureCardField": "preserved"
        })
    }

    fn config(card: Value) -> A2aGatewayConfig {
        A2aGatewayConfig {
            card,
            external_url: None,
            fail_open: false,
        }
    }

    #[test]
    fn accepts_v1_card_and_strict_external_origin() {
        let mut cfg = config(valid_card());
        cfg.external_url = Some("https://agent.example:8443".to_string());
        assert!(cfg.validation_error().is_none());
        assert_eq!(
            cfg.validated_external_origin().unwrap().unwrap().as_str(),
            "https://agent.example:8443/"
        );
    }

    #[test]
    fn rejects_unknown_config_field_but_preserves_unknown_card_fields() {
        let json = json!({ "card": valid_card(), "unexpected": true });
        assert!(serde_json::from_value::<A2aGatewayConfig>(json).is_err());

        let cfg = config(valid_card());
        let bytes = cfg.validated_card_bytes().unwrap();
        let round_trip: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(round_trip["futureCardField"], "preserved");
    }

    #[test]
    fn fail_open_defaults_false_and_accepts_explicit_true() {
        let defaulted: A2aGatewayConfig = serde_json::from_value(json!({ "card": valid_card() })).unwrap();
        assert!(!defaulted.fail_open);

        let enabled: A2aGatewayConfig =
            serde_json::from_value(json!({ "card": valid_card(), "failOpen": true })).unwrap();
        assert!(enabled.fail_open);
    }

    #[test]
    fn rejects_legacy_signed_and_unsupported_cards() {
        let mut legacy = valid_card();
        legacy["url"] = json!("https://legacy.example/rpc");
        assert!(config(legacy).validation_error().unwrap().contains("legacy"));

        let mut signed = valid_card();
        signed["signatures"] = json!([{"protected": "value"}]);
        assert!(config(signed).validation_error().unwrap().contains("signatures"));

        let mut unsupported = valid_card();
        unsupported["supportedInterfaces"][0]["protocolVersion"] = json!("0.3");
        assert!(config(unsupported).validation_error().unwrap().contains("JSONRPC/1.0"));
    }

    #[test]
    fn requires_complete_v1_card_shape() {
        for field in [
            "name",
            "description",
            "version",
            "capabilities",
            "supportedInterfaces",
            "defaultInputModes",
            "defaultOutputModes",
            "skills",
        ] {
            let mut card = valid_card();
            card.as_object_mut().unwrap().remove(field);
            assert!(
                config(card).validation_error().is_some(),
                "accepted card without {field}"
            );
        }

        for field in ["defaultInputModes", "defaultOutputModes", "skills"] {
            let mut card = valid_card();
            card[field] = json!([]);
            assert!(config(card).validation_error().is_some(), "accepted empty {field}");
        }

        for field in ["name", "description", "version"] {
            let mut card = valid_card();
            card[field] = json!("   ");
            assert!(config(card).validation_error().is_some(), "accepted blank {field}");
        }

        let mut capabilities = valid_card();
        capabilities["capabilities"] = json!([]);
        assert!(config(capabilities).validation_error().is_some());

        let mut invalid_mode = valid_card();
        invalid_mode["defaultInputModes"][0] = json!("");
        assert!(config(invalid_mode).validation_error().is_some());
    }

    #[test]
    fn requires_complete_interface_and_skill_shapes() {
        for field in ["url", "protocolBinding", "protocolVersion"] {
            let mut card = valid_card();
            card["supportedInterfaces"][0].as_object_mut().unwrap().remove(field);
            assert!(
                config(card).validation_error().is_some(),
                "accepted interface without {field}"
            );
        }

        for field in ["url", "protocolBinding", "protocolVersion"] {
            let mut card = valid_card();
            card["supportedInterfaces"][0][field] = json!("");
            assert!(
                config(card).validation_error().is_some(),
                "accepted interface with empty {field}"
            );
        }

        for field in ["id", "name", "description", "tags"] {
            let mut card = valid_card();
            card["skills"][0].as_object_mut().unwrap().remove(field);
            assert!(
                config(card).validation_error().is_some(),
                "accepted skill without {field}"
            );
        }

        let mut empty_tags = valid_card();
        empty_tags["skills"][0]["tags"] = json!([]);
        assert!(config(empty_tags).validation_error().is_some());

        let mut empty_tag = valid_card();
        empty_tag["skills"][0]["tags"] = json!([""]);
        assert!(config(empty_tag).validation_error().is_some());
    }

    #[test]
    fn validates_retained_jsonrpc_interface_url_strictly() {
        for invalid in [
            "grpc://agent.example/rpc",
            "https://user:password@agent.example/rpc",
            "https://agent.example/rpc#fragment",
            "https://agent.example/\n/rpc",
        ] {
            let mut card = valid_card();
            card["supportedInterfaces"][0]["url"] = json!(invalid);
            assert!(config(card).validation_error().is_some(), "accepted {invalid:?}");
        }
    }

    #[test]
    fn accepts_non_http_grpc_interface_beside_retained_jsonrpc() {
        let mut card = valid_card();
        card["supportedInterfaces"].as_array_mut().unwrap().push(json!({
            "url": "grpc.example:443",
            "protocolBinding": "GRPC",
            "protocolVersion": "1.0"
        }));

        assert!(config(card).validation_error().is_none());
    }

    #[test]
    fn card_compact_json_limit_is_inclusive() {
        let mut card = valid_card();
        card["padding"] = json!("");
        let base_len = serde_json::to_vec(&card).unwrap().len();
        card["padding"] = json!("x".repeat(A2A_AGENT_CARD_MAX_BYTES - base_len));
        let cfg = config(card.clone());
        assert_eq!(cfg.validated_card_bytes().unwrap().len(), A2A_AGENT_CARD_MAX_BYTES);

        card["padding"] = json!("x".repeat(A2A_AGENT_CARD_MAX_BYTES - base_len + 1));
        assert!(config(card).validation_error().unwrap().contains("must not exceed"));
    }

    #[test]
    fn card_rejects_excessive_json_depth_and_nodes_before_serialization() {
        use super::super::json_schema_validation::{MAX_JSON_INSTANCE_DEPTH, MAX_JSON_INSTANCE_NODES};

        let mut deep = Value::Null;
        for _ in 0..=MAX_JSON_INSTANCE_DEPTH {
            deep = json!([deep]);
        }
        let mut deep_card = valid_card();
        deep_card["extension"] = deep;
        assert!(config(deep_card).validation_error().unwrap().contains("depth or node"));

        let mut wide_card = valid_card();
        wide_card["extension"] = Value::Array(vec![Value::Null; MAX_JSON_INSTANCE_NODES as usize]);
        assert!(config(wide_card).validation_error().unwrap().contains("depth or node"));
    }

    #[test]
    fn external_url_is_origin_only() {
        for invalid in [
            "agent.example",
            "ftp://agent.example",
            "https://agent.example/rpc",
            "https://agent.example/?tenant=blue",
            "https://agent.example/#fragment",
            "https://user@agent.example",
        ] {
            let mut cfg = config(valid_card());
            cfg.external_url = Some(invalid.to_string());
            assert!(cfg.validation_error().is_some(), "accepted {invalid:?}");
        }
    }
}
