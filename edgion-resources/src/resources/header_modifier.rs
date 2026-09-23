//! Shared admission checks for Gateway API header modifiers.
//!
//! Validate before publication in both Controller and Gateway. Errors never
//! include configured names or values, which may contain credentials.

use crate::response_header_policy::validate_response_header_mutation;
use http::HeaderValue;

use super::grpc_route::{GRPCHeaderFilter, GRPCRouteFilter, GRPCRouteFilterType, GRPCRouteRule};
use super::http_route::{HTTPHeader, HTTPHeaderFilter, HTTPRouteFilter, HTTPRouteFilterType, HTTPRouteRule};
use crate::gwapi_types::{is_valid_header_token, HEADER_NAME_MAX_LEN};

const MAX_HEADER_ACTIONS: usize = 16;
const MAX_HEADER_VALUE_CHARS: usize = 4096;

fn validate_name(name: &str) -> Result<(), &'static str> {
    if name.len() > HEADER_NAME_MAX_LEN as usize || !is_valid_header_token(name) {
        return Err("header modifier names must be HTTP tokens of 1 to 256 bytes");
    }
    Ok(())
}

fn validate_actions(set: &[HTTPHeader], add: &[HTTPHeader], remove: &[String]) -> Result<(), &'static str> {
    if set.len() > MAX_HEADER_ACTIONS || add.len() > MAX_HEADER_ACTIONS || remove.len() > MAX_HEADER_ACTIONS {
        return Err("header modifier set, add, and remove lists must each contain at most 16 entries");
    }
    // Validate even entries ignored by first-equivalent execution. Admission
    // applies to the entire configuration, not only the eventual runtime plan.
    for header in set.iter().chain(add) {
        validate_name(&header.name)?;
        // Standard Gateway API string bounds count Unicode scalar values.
        // Transport parsing additionally excludes CR, LF, and other controls.
        if header.value.is_empty()
            || header.value.chars().take(MAX_HEADER_VALUE_CHARS + 1).count() > MAX_HEADER_VALUE_CHARS
        {
            return Err("header modifier values must contain 1 to 4096 characters");
        }
        HeaderValue::from_bytes(header.value.as_bytes())
            .map_err(|_| "header modifier values must be valid HTTP header values")?;
    }
    for name in remove {
        validate_name(name)?;
    }
    // Each list has at most 16 entries. Bounded comparisons avoid allocating
    // normalized names and preserve first-equivalent behavior within one list.
    if set
        .iter()
        .any(|entry| add.iter().any(|other| entry.name.eq_ignore_ascii_case(&other.name)))
        || remove.iter().any(|name| {
            set.iter()
                .chain(add)
                .any(|entry| name.eq_ignore_ascii_case(&entry.name))
        })
    {
        return Err("header modifier names must not appear in more than one of set, add, and remove");
    }
    Ok(())
}

impl HTTPHeaderFilter {
    /// Validate ordinary response mutations in addition to the shared syntax.
    pub fn validate_response(&self) -> Result<(), &'static str> {
        self.validate()?;
        validate_response_actions(
            self.set.as_deref().unwrap_or_default(),
            self.add.as_deref().unwrap_or_default(),
            self.remove.as_deref().unwrap_or_default(),
        )
    }
    /// Validate the standard Gateway API header shape and HTTP transport syntax.
    pub fn validate(&self) -> Result<(), &'static str> {
        validate_actions(
            self.set.as_deref().unwrap_or_default(),
            self.add.as_deref().unwrap_or_default(),
            self.remove.as_deref().unwrap_or_default(),
        )
    }
}

impl GRPCHeaderFilter {
    /// Response fields follow the same ownership policy for HTTP and gRPC.
    pub fn validate_response(&self) -> Result<(), &'static str> {
        self.validate()?;
        validate_response_actions(
            self.set.as_deref().unwrap_or_default(),
            self.add.as_deref().unwrap_or_default(),
            self.remove.as_deref().unwrap_or_default(),
        )
    }
    /// Apply the same header contract as HTTPRoute and EdgionPlugins.
    pub fn validate(&self) -> Result<(), &'static str> {
        validate_actions(
            self.set.as_deref().unwrap_or_default(),
            self.add.as_deref().unwrap_or_default(),
            self.remove.as_deref().unwrap_or_default(),
        )
    }
}

fn validate_response_actions(set: &[HTTPHeader], add: &[HTTPHeader], remove: &[String]) -> Result<(), &'static str> {
    for name in set
        .iter()
        .chain(add)
        .map(|header| header.name.as_str())
        .chain(remove.iter().map(String::as_str))
    {
        validate_response_header_mutation(name)?;
    }
    Ok(())
}

fn validate_pairing(
    request_type: bool,
    response_type: bool,
    request_config: bool,
    response_config: bool,
    other_config: bool,
) -> Result<(), &'static str> {
    if request_type != request_config
        || response_type != response_config
        || ((request_type || response_type) && other_config)
    {
        return Err("header modifier filter type must match exactly its configuration field");
    }
    Ok(())
}

fn validate_repetition(
    request_type: bool,
    response_type: bool,
    seen_request: &mut bool,
    seen_response: &mut bool,
) -> Result<(), &'static str> {
    if (request_type && *seen_request) || (response_type && *seen_response) {
        return Err("request and response header modifiers must each occur at most once per filter list");
    }
    *seen_request |= request_type;
    *seen_response |= response_type;
    Ok(())
}

/// Validate header modifiers in one native HTTP rule or backend filter list.
pub fn validate_http_filter_list(filters: &[HTTPRouteFilter]) -> Result<(), &'static str> {
    let (mut seen_request, mut seen_response) = (false, false);
    for filter in filters {
        let request = filter.filter_type == HTTPRouteFilterType::RequestHeaderModifier;
        let response = filter.filter_type == HTTPRouteFilterType::ResponseHeaderModifier;
        validate_pairing(
            request,
            response,
            filter.request_header_modifier.is_some(),
            filter.response_header_modifier.is_some(),
            filter.request_redirect.is_some()
                || filter.url_rewrite.is_some()
                || filter.request_mirror.is_some()
                || filter.cors.is_some()
                || filter.external_auth.is_some()
                || filter.extension_ref.is_some(),
        )?;
        validate_repetition(request, response, &mut seen_request, &mut seen_response)?;
        if let Some(config) = &filter.request_header_modifier {
            config.validate()?;
        }
        if let Some(config) = &filter.response_header_modifier {
            config.validate_response()?;
        }
    }
    Ok(())
}

/// Validate header modifiers in one native gRPC rule or backend filter list.
pub fn validate_grpc_filter_list(filters: &[GRPCRouteFilter]) -> Result<(), &'static str> {
    let (mut seen_request, mut seen_response) = (false, false);
    for filter in filters {
        let request = filter.filter_type == GRPCRouteFilterType::RequestHeaderModifier;
        let response = filter.filter_type == GRPCRouteFilterType::ResponseHeaderModifier;
        validate_pairing(
            request,
            response,
            filter.request_header_modifier.is_some(),
            filter.response_header_modifier.is_some(),
            filter.extension_ref.is_some(),
        )?;
        validate_repetition(request, response, &mut seen_request, &mut seen_response)?;
        if let Some(config) = &filter.request_header_modifier {
            config.validate()?;
        }
        if let Some(config) = &filter.response_header_modifier {
            config.validate_response()?;
        }
    }
    Ok(())
}

impl HTTPRouteRule {
    /// Report the first invalid header modifier in this rule or its backends.
    pub fn header_modifier_error(&self) -> Option<&'static str> {
        validate_http_filter_list(self.filters.as_deref().unwrap_or_default())
            .err()
            .or_else(|| {
                self.backend_refs
                    .iter()
                    .flatten()
                    .find_map(|backend| validate_http_filter_list(backend.filters.as_deref().unwrap_or_default()).err())
            })
    }
}

impl GRPCRouteRule {
    /// Report the first invalid header modifier in this rule or its backends.
    pub fn header_modifier_error(&self) -> Option<&'static str> {
        validate_grpc_filter_list(self.filters.as_deref().unwrap_or_default())
            .err()
            .or_else(|| {
                self.backend_refs
                    .iter()
                    .flatten()
                    .find_map(|backend| validate_grpc_filter_list(backend.filters.as_deref().unwrap_or_default()).err())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn assert_config(config: Value, valid: bool) {
        let http: HTTPHeaderFilter = serde_json::from_value(config.clone()).unwrap();
        let grpc: GRPCHeaderFilter = serde_json::from_value(config.clone()).unwrap();
        assert_eq!(http.validate().is_ok(), valid, "HTTP config: {config}");
        assert_eq!(grpc.validate(), http.validate(), "HTTP/gRPC parity: {config}");
    }

    #[test]
    fn header_modifier_names_and_values_use_shared_http_grpc_contract() {
        for name in [
            "",
            "X Tenant",
            ":authority",
            "x:bad",
            "x\rname",
            "x\nname",
            "x\0name",
            "é",
        ] {
            for field in ["set", "add"] {
                assert_config(json!({field: [{"name": name, "value": "valid"}]}), false);
            }
            assert_config(json!({"remove": [name]}), false);
        }
        assert_config(json!({"remove": ["x".repeat(257)]}), false);
        assert_config(json!({"set": [{"name": "x".repeat(257), "value": "valid"}]}), false);
        for value in [
            "",
            "line\rbreak",
            "line\nbreak",
            "nul\0byte",
            "del\u{7f}",
            "control\u{1f}",
        ] {
            for field in ["set", "add"] {
                assert_config(json!({field: [{"name": "x-test", "value": value}]}), false);
            }
        }
        assert_config(json!({"add": [{"name": "x-test", "value": "x".repeat(4097)}]}), false);
        assert_config(json!({"set": [{"name": "x-test", "value": "é".repeat(4097)}]}), false);
    }

    #[test]
    fn header_modifier_accepts_standard_boundaries_and_literal_values() {
        assert_config(json!({}), true);
        assert_config(json!({"set": [], "add": [], "remove": []}), true);
        assert_config(
            json!({"set": [{"name": "x".repeat(256), "value": "x".repeat(4096)}]}),
            true,
        );
        assert_config(
            json!({"add": [{"name": "!#$%&'*+-.^_`|~Az09", "value": "é".repeat(4096)}]}),
            true,
        );
        assert_config(
            json!({"set": [{"name": "X-Literal", "value": " ${request:value}\t "}], "remove": ["x".repeat(256)]}),
            true,
        );
        for count in [16, 17] {
            for field in ["set", "add"] {
                let entries: Vec<_> = (0..count)
                    .map(|i| json!({"name": format!("x-{i}"), "value": "ok"}))
                    .collect();
                assert_config(json!({field: entries}), count == 16);
            }
            let names: Vec<_> = (0..count).map(|i| format!("x-{i}")).collect();
            assert_config(json!({"remove": names}), count == 16);
        }
    }

    #[test]
    fn header_modifier_checks_all_duplicates_and_cross_action_conflicts() {
        for field in ["set", "add"] {
            assert_config(
                json!({field: [{"name": "x-test", "value": "first"}, {"name": "X-Test", "value": "second"}]}),
                true,
            );
            assert_config(
                json!({field: [{"name": "x-test", "value": "first"}, {"name": "X-Test", "value": "bad\nvalue"}]}),
                false,
            );
        }
        assert_config(json!({"remove": ["x-test", "X-Test"]}), true);
        assert_config(
            json!({"set": [{"name": "x-test", "value": "ok"}], "add": [{"name": "X-Test", "value": "other"}]}),
            false,
        );
        for field in ["set", "add"] {
            assert_config(
                json!({field: [{"name": "x-test", "value": "ok"}], "remove": ["X-Test"]}),
                false,
            );
        }
        assert_config(
            json!({"set": [{"name": "x-set", "value": "ok"}], "add": [{"name": "x-add", "value": "ok"}], "remove": ["x-remove"]}),
            true,
        );
    }

    fn assert_lists(filters: Value, valid: bool) {
        let http: Vec<HTTPRouteFilter> = serde_json::from_value(filters.clone()).unwrap();
        let grpc: Vec<GRPCRouteFilter> = serde_json::from_value(filters.clone()).unwrap();
        assert_eq!(validate_http_filter_list(&http).is_ok(), valid, "filters: {filters}");
        assert_eq!(validate_grpc_filter_list(&grpc), validate_http_filter_list(&http));
        for rule in [
            json!({"filters": filters}),
            json!({"backendRefs": [{"name": "backend", "port": 80, "filters": filters}]}),
        ] {
            let http: HTTPRouteRule = serde_json::from_value(rule.clone()).unwrap();
            let grpc: GRPCRouteRule = serde_json::from_value(rule).unwrap();
            assert_eq!(http.header_modifier_error().is_none(), valid);
            assert_eq!(grpc.header_modifier_error(), http.header_modifier_error());
        }
    }

    #[test]
    fn response_header_policy_is_directional_for_native_rules_and_backends() {
        for name in ["Content-Length", "cOnNeCtIoN", "Proxy-Connection", "TE"] {
            for (kind, field, valid) in [
                ("ResponseHeaderModifier", "responseHeaderModifier", false),
                ("RequestHeaderModifier", "requestHeaderModifier", true),
            ] {
                for action in ["set", "add", "remove"] {
                    let value = if action == "remove" {
                        json!([name])
                    } else {
                        json!([{"name":name, "value":"1"}])
                    };
                    assert_lists(json!([{"type":kind, field:{action:value}}]), valid);
                }
            }
        }
    }

    #[test]
    fn header_modifier_native_pairing_and_repetition_cover_rule_and_backend_lists() {
        assert_lists(json!([]), true);
        assert_lists(
            json!([{"type": "RequestHeaderModifier", "requestHeaderModifier": {}}, {"type": "ResponseHeaderModifier", "responseHeaderModifier": {}}]),
            true,
        );
        for (kind, field, other) in [
            (
                "RequestHeaderModifier",
                "requestHeaderModifier",
                "responseHeaderModifier",
            ),
            (
                "ResponseHeaderModifier",
                "responseHeaderModifier",
                "requestHeaderModifier",
            ),
        ] {
            assert_lists(json!([{"type": kind}]), false);
            assert_lists(json!([{"type": kind, other: {}}]), false);
            assert_lists(json!([{"type": kind, field: {}, other: {}}]), false);
            assert_lists(
                json!([{"type": kind, field: {}, "extensionRef": {"group": "edgion.io", "kind": "EdgionPlugins", "name": "plugins"}}]),
                false,
            );
            assert_lists(json!([{"type": "ExtensionRef", field: {}}]), false);
            assert_lists(json!([{"type": kind, field: {}}, {"type": kind, field: {}}]), false);
            assert_lists(
                json!([{"type": kind, field: {"set": [{"name": "x-test", "value": "invalid\nvalue"}]}}]),
                false,
            );
        }
    }

    #[test]
    fn header_modifier_http_rejects_unrelated_payload_on_header_filter() {
        for (field, value) in [
            ("requestRedirect", json!({})),
            ("urlRewrite", json!({})),
            ("requestMirror", json!({"backendRef": {"name": "mirror", "port": 80}})),
            ("cors", json!({})),
            (
                "externalAuth",
                json!({"target": {"name": "auth", "port": 8080}, "decision": {}}),
            ),
        ] {
            let filters: Vec<HTTPRouteFilter> = serde_json::from_value(
                json!([{"type": "RequestHeaderModifier", "requestHeaderModifier": {}, field: value}]),
            )
            .unwrap();
            assert!(validate_http_filter_list(&filters).is_err(), "unrelated field: {field}");
        }
    }
}
