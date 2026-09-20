//! Gateway API request-transform validation and immutable preparation.

use super::{
    HTTPPathMatch, HTTPPathModifier, HTTPPathModifierType, HTTPRequestRedirectFilter, HTTPRouteFilter,
    HTTPRouteFilterType, HTTPRouteMatch, HTTPRouteRule, HTTPURLRewriteFilter,
};
use crate::crd_contract::{http_route_transform_contract, CrdContractError, CrdViolation};
use http::{uri::PathAndQuery, StatusCode};
use serde_json::{Map, Value};
use std::{num::NonZeroU16, str::FromStr};

/// A schema or execution-boundary failure for a request transform.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestTransformViolation {
    pub path: String,
    pub message: String,
}

impl RequestTransformViolation {
    fn contract(prefix: &str, violation: CrdViolation) -> Self {
        Self {
            path: format!("{prefix}{}", violation.instance_path),
            message: violation.message,
        }
    }

    fn static_message(path: impl Into<String>, message: &'static str) -> Self {
        Self {
            path: path.into(),
            message: message.to_string(),
        }
    }

    fn prefixed(prefix: &str, mut violation: Self) -> Self {
        violation.path = match (prefix.is_empty(), violation.path.is_empty()) {
            (true, _) => violation.path,
            (false, true) => prefix.to_string(),
            (false, false) => format!("{prefix}/{}", violation.path),
        };
        violation
    }
}

impl std::fmt::Display for RequestTransformViolation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.path.is_empty() {
            formatter.write_str(&self.message)
        } else {
            write!(formatter, "{}: {}", self.path, self.message)
        }
    }
}

/// Scheme already restricted to the Gateway API RequestRedirect contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparedRedirectScheme {
    Http,
    Https,
}

impl PreparedRedirectScheme {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }
}

/// Path mutation whose type/config pairing and URI transport shape are valid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreparedPathModifier {
    ReplaceFullPath(String),
    ReplacePrefixMatch(String),
}

/// Immutable redirect state consumed by the Gateway request path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedRequestRedirect {
    scheme: Option<PreparedRedirectScheme>,
    hostname: Option<String>,
    path: Option<PreparedPathModifier>,
    port: Option<NonZeroU16>,
    status_code: StatusCode,
}

impl PreparedRequestRedirect {
    pub fn scheme(&self) -> Option<PreparedRedirectScheme> {
        self.scheme
    }

    pub fn hostname(&self) -> Option<&str> {
        self.hostname.as_deref()
    }

    pub fn path(&self) -> Option<&PreparedPathModifier> {
        self.path.as_ref()
    }

    pub fn port(&self) -> Option<NonZeroU16> {
        self.port
    }

    pub fn status_code(&self) -> StatusCode {
        self.status_code
    }
}

/// Immutable URLRewrite state consumed by the Gateway request path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedUrlRewrite {
    hostname: Option<String>,
    path: Option<PreparedPathModifier>,
}

impl PreparedUrlRewrite {
    pub fn hostname(&self) -> Option<&str> {
        self.hostname.as_deref()
    }

    pub fn path(&self) -> Option<&PreparedPathModifier> {
        self.path.as_ref()
    }
}

pub fn authored_request_transform_violations(
    rule: &HTTPRouteRule,
) -> Result<Vec<RequestTransformViolation>, CrdContractError> {
    let contract = http_route_transform_contract()?;
    let mut violations = validate_rule_filter_lists(rule, true)?;
    let rule_value = normalized_rule_value(rule)?;
    violations.extend(
        contract
            .validate_rule(&rule_value)?
            .into_iter()
            .map(|violation| RequestTransformViolation::contract("", violation)),
    );
    validate_prefix_context(rule, &mut violations);
    Ok(violations)
}

pub fn request_transform_filter_list_violations(
    filters: &[HTTPRouteFilter],
) -> Result<Vec<RequestTransformViolation>, CrdContractError> {
    let mut violations = Vec::new();
    validate_filter_list(Some(filters), "filters", false, &mut violations)?;
    Ok(violations)
}

/// Validate an effective rule after delegation expansion.
///
/// Authored-only cardinality and exactly-one-match CEL are deliberately not
/// applied. Every effective match must instead provide a PathPrefix context
/// whenever a prefix transform can execute.
pub fn effective_request_transform_violations(
    rule: &HTTPRouteRule,
) -> Result<Vec<RequestTransformViolation>, CrdContractError> {
    let mut violations = validate_rule_filter_lists(rule, false)?;
    if rule_has_redirect(rule.filters.as_deref()) && rule.backend_refs.as_ref().is_some_and(|refs| !refs.is_empty()) {
        violations.push(RequestTransformViolation::static_message(
            "filters",
            "RequestRedirect filter must not be used together with backendRefs",
        ));
    }
    validate_prefix_context(rule, &mut violations);
    Ok(violations)
}

fn validate_prefix_context(rule: &HTTPRouteRule, violations: &mut Vec<RequestTransformViolation>) {
    if rule_has_prefix_transform(rule) && !effective_matches_are_prefix(rule) {
        violations.push(RequestTransformViolation::static_message(
            "matches",
            "prefix request transforms require PathPrefix match context",
        ));
    }
}

fn validate_rule_filter_lists(
    rule: &HTTPRouteRule,
    authored: bool,
) -> Result<Vec<RequestTransformViolation>, CrdContractError> {
    let mut violations = Vec::new();
    validate_filter_list(rule.filters.as_deref(), "filters", authored, &mut violations)?;
    for (backend_index, backend) in rule.backend_refs.iter().flatten().enumerate() {
        validate_filter_list(
            backend.filters.as_deref(),
            &format!("backendRefs/{backend_index}/filters"),
            authored,
            &mut violations,
        )?;
    }

    let has_rule_redirect = rule_has_redirect(rule.filters.as_deref());
    let has_rule_rewrite = rule_has_rewrite(rule.filters.as_deref());
    for (backend_index, backend) in rule.backend_refs.iter().flatten().enumerate() {
        let backend_has_redirect = rule_has_redirect(backend.filters.as_deref());
        let backend_has_rewrite = rule_has_rewrite(backend.filters.as_deref());
        if (has_rule_redirect && backend_has_rewrite) || (has_rule_rewrite && backend_has_redirect) {
            violations.push(RequestTransformViolation::static_message(
                format!("backendRefs/{backend_index}/filters"),
                "RequestRedirect and URLRewrite filters must not be combined across rule and backend scopes",
            ));
        }
    }
    Ok(violations)
}

fn validate_filter_list(
    filters: Option<&[HTTPRouteFilter]>,
    prefix: &str,
    authored: bool,
    violations: &mut Vec<RequestTransformViolation>,
) -> Result<(), CrdContractError> {
    let Some(filters) = filters else {
        return Ok(());
    };
    let contract = http_route_transform_contract()?;
    let list_value = transform_filter_list_value(filters)?;
    if authored {
        violations.extend(
            contract
                .validate_filter_list_bounds(&list_value)?
                .into_iter()
                .map(|violation| RequestTransformViolation::contract(prefix, violation)),
        );
    }
    violations.extend(
        contract
            .validate_filter_list(&list_value)?
            .into_iter()
            .map(|violation| RequestTransformViolation::contract(prefix, violation)),
    );
    for (index, filter) in filters.iter().enumerate() {
        let filter_prefix = format!("{prefix}/{index}");
        let filter_value = list_value.get(index).ok_or(CrdContractError::InvalidContract)?;
        violations.extend(
            contract
                .validate_filter(filter_value)?
                .into_iter()
                .map(|violation| RequestTransformViolation::contract(&filter_prefix, violation)),
        );
        if let Some(config) = filter.request_redirect.as_ref() {
            let value = serde_json::to_value(config).map_err(|_| CrdContractError::InvalidContract)?;
            let config_prefix = format!("{filter_prefix}/requestRedirect");
            let config_violations = contract.validate_request_redirect(&value)?;
            let config_is_valid = config_violations.is_empty();
            violations.extend(
                config_violations
                    .into_iter()
                    .map(|violation| RequestTransformViolation::contract(&config_prefix, violation)),
            );
            if config_is_valid {
                if let Err(violation) = config.prepare_validated(contract.defaults().redirect_status_code) {
                    violations.push(RequestTransformViolation::prefixed(&config_prefix, violation));
                }
            }
        }
        if let Some(config) = filter.url_rewrite.as_ref() {
            let value = serde_json::to_value(config).map_err(|_| CrdContractError::InvalidContract)?;
            let config_prefix = format!("{filter_prefix}/urlRewrite");
            let config_violations = contract.validate_url_rewrite(&value)?;
            let config_is_valid = config_violations.is_empty();
            violations.extend(
                config_violations
                    .into_iter()
                    .map(|violation| RequestTransformViolation::contract(&config_prefix, violation)),
            );
            if config_is_valid {
                if let Err(violation) = config.prepare_validated() {
                    violations.push(RequestTransformViolation::prefixed(&config_prefix, violation));
                }
            }
        }
    }
    Ok(())
}

fn normalized_rule_value(rule: &HTTPRouteRule) -> Result<Value, CrdContractError> {
    let contract = http_route_transform_contract()?;
    let mut object = Map::new();
    let matches = match rule.matches.as_deref() {
        None => contract.defaults().matches.clone(),
        Some(matches) => Value::Array(
            matches
                .iter()
                .map(|route_match| {
                    let path = match route_match.path.as_ref() {
                        None => contract.defaults().path.clone(),
                        Some(path) => {
                            let mut path_object = Map::new();
                            path_object.insert(
                                "type".to_string(),
                                Value::String(effective_path_match_type(path).to_string()),
                            );
                            path_object.insert(
                                "value".to_string(),
                                Value::String(effective_path_match_value(path).to_string()),
                            );
                            Value::Object(path_object)
                        }
                    };
                    Value::Object(Map::from_iter([("path".to_string(), path)]))
                })
                .collect(),
        ),
    };
    object.insert("matches".to_string(), matches);
    if let Some(filters) = rule.filters.as_deref() {
        object.insert("filters".to_string(), transform_filter_list_value(filters)?);
    }
    if let Some(backends) = rule.backend_refs.as_deref() {
        let backends = backends
            .iter()
            .map(|backend| {
                let mut backend_object = Map::new();
                if let Some(filters) = backend.filters.as_deref() {
                    backend_object.insert("filters".to_string(), transform_filter_list_value(filters)?);
                }
                Ok(Value::Object(backend_object))
            })
            .collect::<Result<Vec<_>, CrdContractError>>()?;
        object.insert("backendRefs".to_string(), Value::Array(backends));
    }
    Ok(Value::Object(object))
}

fn transform_filter_list_value(filters: &[HTTPRouteFilter]) -> Result<Value, CrdContractError> {
    filters
        .iter()
        .map(|filter| {
            let mut object = Map::new();
            object.insert(
                "type".to_string(),
                serde_json::to_value(&filter.filter_type).map_err(|_| CrdContractError::InvalidContract)?,
            );
            if let Some(config) = filter.request_redirect.as_ref() {
                object.insert(
                    "requestRedirect".to_string(),
                    serde_json::to_value(config).map_err(|_| CrdContractError::InvalidContract)?,
                );
            }
            if let Some(config) = filter.url_rewrite.as_ref() {
                object.insert(
                    "urlRewrite".to_string(),
                    serde_json::to_value(config).map_err(|_| CrdContractError::InvalidContract)?,
                );
            }
            Ok(Value::Object(object))
        })
        .collect::<Result<Vec<_>, CrdContractError>>()
        .map(Value::Array)
}

fn rule_has_redirect(filters: Option<&[HTTPRouteFilter]>) -> bool {
    filters
        .unwrap_or_default()
        .iter()
        .any(|filter| filter.filter_type == HTTPRouteFilterType::RequestRedirect)
}

fn rule_has_rewrite(filters: Option<&[HTTPRouteFilter]>) -> bool {
    filters
        .unwrap_or_default()
        .iter()
        .any(|filter| filter.filter_type == HTTPRouteFilterType::URLRewrite)
}

fn rule_has_prefix_transform(rule: &HTTPRouteRule) -> bool {
    rule.filters
        .iter()
        .flatten()
        .chain(
            rule.backend_refs
                .iter()
                .flatten()
                .flat_map(|backend| backend.filters.iter().flatten()),
        )
        .any(|filter| {
            filter
                .request_redirect
                .as_ref()
                .and_then(|config| config.path.as_ref())
                .or_else(|| filter.url_rewrite.as_ref().and_then(|config| config.path.as_ref()))
                .is_some_and(|path| path.modifier_type == HTTPPathModifierType::ReplacePrefixMatch)
        })
}

fn effective_matches_are_prefix(rule: &HTTPRouteRule) -> bool {
    let matches = effective_matches(rule);
    !matches.is_empty()
        && matches.iter().all(|route_match| {
            route_match
                .path
                .as_ref()
                .is_none_or(|path| effective_path_match_type(path) == "PathPrefix")
        })
}

fn effective_matches(rule: &HTTPRouteRule) -> Vec<HTTPRouteMatch> {
    match rule.matches.as_ref() {
        Some(matches) => matches.clone(),
        None => vec![HTTPRouteMatch {
            path: Some(default_path_match()),
            headers: None,
            query_params: None,
            method: None,
        }],
    }
}

pub fn default_path_match() -> HTTPPathMatch {
    HTTPPathMatch {
        match_type: default_path_match_type(),
        value: default_path_match_value(),
    }
}

pub fn default_path_match_type() -> Option<String> {
    http_route_transform_contract()
        .ok()
        .map(|contract| contract.defaults().path_type.clone())
}

pub fn default_path_match_value() -> Option<String> {
    http_route_transform_contract()
        .ok()
        .map(|contract| contract.defaults().path_value.clone())
}

pub fn effective_path_match_type(path: &HTTPPathMatch) -> &str {
    path.match_type.as_deref().unwrap_or_else(|| {
        http_route_transform_contract()
            .ok()
            .map(|contract| contract.defaults().path_type.as_str())
            .unwrap_or("")
    })
}

pub fn effective_path_match_value(path: &HTTPPathMatch) -> &str {
    path.value.as_deref().unwrap_or_else(|| {
        http_route_transform_contract()
            .ok()
            .map(|contract| contract.defaults().path_value.as_str())
            .unwrap_or("")
    })
}

impl HTTPRequestRedirectFilter {
    pub fn prepare(&self) -> Result<PreparedRequestRedirect, RequestTransformViolation> {
        let contract = http_route_transform_contract().map_err(|_| {
            RequestTransformViolation::static_message("", "request transform validation contract is unavailable")
        })?;
        let value = serde_json::to_value(self).map_err(|_| {
            RequestTransformViolation::static_message("", "request redirect configuration could not be validated")
        })?;
        if let Some(violation) = contract
            .validate_request_redirect(&value)
            .map_err(|_| RequestTransformViolation::static_message("", "request redirect validation failed"))?
            .into_iter()
            .next()
        {
            return Err(RequestTransformViolation::contract("", violation));
        }
        self.prepare_validated(contract.defaults().redirect_status_code)
    }

    fn prepare_validated(
        &self,
        default_status_code: i32,
    ) -> Result<PreparedRequestRedirect, RequestTransformViolation> {
        let scheme = match self.scheme.as_deref() {
            None => None,
            Some("http") => Some(PreparedRedirectScheme::Http),
            Some("https") => Some(PreparedRedirectScheme::Https),
            Some(_) => {
                return Err(RequestTransformViolation::static_message(
                    "scheme",
                    "redirect scheme is not supported",
                ))
            }
        };
        let port = match self.port {
            None => None,
            Some(port) => Some(
                u16::try_from(port)
                    .ok()
                    .and_then(NonZeroU16::new)
                    .ok_or_else(|| RequestTransformViolation::static_message("port", "redirect port is invalid"))?,
            ),
        };
        let status = self.status_code.unwrap_or(default_status_code);
        let status_code = u16::try_from(status)
            .ok()
            .and_then(|status| StatusCode::from_u16(status).ok())
            .ok_or_else(|| {
                RequestTransformViolation::static_message("statusCode", "redirect status code is invalid")
            })?;
        Ok(PreparedRequestRedirect {
            scheme,
            hostname: self.hostname.clone(),
            path: self.path.as_ref().map(prepare_path_modifier).transpose()?,
            port,
            status_code,
        })
    }
}

impl HTTPURLRewriteFilter {
    pub fn prepare(&self) -> Result<PreparedUrlRewrite, RequestTransformViolation> {
        let contract = http_route_transform_contract().map_err(|_| {
            RequestTransformViolation::static_message("", "request transform validation contract is unavailable")
        })?;
        let value = serde_json::to_value(self).map_err(|_| {
            RequestTransformViolation::static_message("", "URL rewrite configuration could not be validated")
        })?;
        if let Some(violation) = contract
            .validate_url_rewrite(&value)
            .map_err(|_| RequestTransformViolation::static_message("", "URL rewrite validation failed"))?
            .into_iter()
            .next()
        {
            return Err(RequestTransformViolation::contract("", violation));
        }
        self.prepare_validated()
    }

    fn prepare_validated(&self) -> Result<PreparedUrlRewrite, RequestTransformViolation> {
        Ok(PreparedUrlRewrite {
            hostname: self.hostname.clone(),
            path: self.path.as_ref().map(prepare_path_modifier).transpose()?,
        })
    }
}

fn prepare_path_modifier(path: &HTTPPathModifier) -> Result<PreparedPathModifier, RequestTransformViolation> {
    match path.modifier_type {
        HTTPPathModifierType::ReplaceFullPath => {
            let value = path.replace_full_path.as_deref().ok_or_else(|| {
                RequestTransformViolation::static_message("path/replaceFullPath", "replacement path is required")
            })?;
            let value = if value.is_empty() { "/" } else { value };
            validate_transport_path(value, "path/replaceFullPath")?;
            Ok(PreparedPathModifier::ReplaceFullPath(value.to_string()))
        }
        HTTPPathModifierType::ReplacePrefixMatch => {
            let value = path.replace_prefix_match.as_deref().ok_or_else(|| {
                RequestTransformViolation::static_message("path/replacePrefixMatch", "replacement prefix is required")
            })?;
            if !value.is_empty() {
                validate_transport_path(value, "path/replacePrefixMatch")?;
            }
            Ok(PreparedPathModifier::ReplacePrefixMatch(value.to_string()))
        }
    }
}

fn validate_transport_path(value: &str, path: &str) -> Result<(), RequestTransformViolation> {
    if !value.starts_with('/') || value.contains('?') || value.contains('#') || PathAndQuery::from_str(value).is_err() {
        return Err(RequestTransformViolation::static_message(
            path.to_string(),
            "replacement must be an absolute HTTP path without query or fragment",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::edgion_plugins::EdgionPlugin;
    use serde_json::json;

    fn rule(value: Value) -> HTTPRouteRule {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn authored_contract_rejects_pairing_repetition_and_conflicts() {
        let invalid = rule(json!({
            "filters": [
                {"type": "RequestRedirect"},
                {"type": "RequestRedirect", "requestRedirect": {}},
                {"type": "URLRewrite", "urlRewrite": {}}
            ]
        }));
        let violations = authored_request_transform_violations(&invalid).unwrap();
        assert!(violations
            .iter()
            .any(|violation| violation.message.contains("must be specified")));
        assert!(violations
            .iter()
            .any(|violation| violation.message.contains("cannot be repeated")));
        assert!(violations
            .iter()
            .any(|violation| violation.message.contains("not both")));
    }

    #[test]
    fn authored_contract_enforces_the_generated_filter_limit() {
        let filters = (0..17)
            .map(|_| json!({"type": "RequestHeaderModifier", "requestHeaderModifier": {}}))
            .collect::<Vec<_>>();
        let violations = authored_request_transform_violations(&rule(json!({"filters": filters}))).unwrap();
        assert!(violations.iter().any(|violation| violation.path == "filters"));
    }

    #[test]
    fn unrelated_large_match_data_is_outside_the_transform_budget() {
        let headers = (0..16)
            .map(|index| {
                json!({
                    "name": format!("x-large-{index}"),
                    "value": "x".repeat(4096)
                })
            })
            .collect::<Vec<_>>();
        let matches = (0..5)
            .map(|index| {
                json!({
                    "path": {"type": "Exact", "value": format!("/item-{index}")},
                    "headers": headers
                })
            })
            .collect::<Vec<_>>();
        let rule = rule(json!({"matches": matches}));

        assert!(authored_request_transform_violations(&rule).unwrap().is_empty());
        assert!(effective_request_transform_violations(&rule).unwrap().is_empty());
    }

    #[test]
    fn authored_contract_uses_match_defaults_for_prefix_rewrite() {
        let valid = rule(json!({
            "filters": [{"type": "URLRewrite", "urlRewrite": {"path": {
                "type": "ReplacePrefixMatch", "replacePrefixMatch": "/v2"
            }}}]
        }));
        assert!(authored_request_transform_violations(&valid).unwrap().is_empty());

        let invalid = rule(json!({
            "matches": [{"path": {"type": "Exact", "value": "/api"}}],
            "filters": [{"type": "URLRewrite", "urlRewrite": {"path": {
                "type": "ReplacePrefixMatch", "replacePrefixMatch": "/v2"
            }}}]
        }));
        assert!(!authored_request_transform_violations(&invalid).unwrap().is_empty());
    }

    #[test]
    fn authored_and_effective_validation_include_transport_preparation() {
        let invalid = rule(json!({
            "filters": [{"type": "URLRewrite", "urlRewrite": {"path": {
                "type": "ReplaceFullPath", "replaceFullPath": "relative"
            }}}]
        }));
        for violations in [
            authored_request_transform_violations(&invalid).unwrap(),
            effective_request_transform_violations(&invalid).unwrap(),
        ] {
            assert!(violations.iter().any(|violation| {
                violation.path == "filters/0/urlRewrite/path/replaceFullPath"
                    && violation.message.contains("absolute HTTP path")
            }));
        }
    }

    #[test]
    fn prepared_redirect_is_typed_and_transport_safe() {
        let config: HTTPRequestRedirectFilter = serde_json::from_value(json!({
            "scheme": "https", "port": 8443, "statusCode": 307,
            "path": {"type": "ReplaceFullPath", "replaceFullPath": "/login"}
        }))
        .unwrap();
        let prepared = config.prepare().unwrap();
        assert_eq!(prepared.scheme(), Some(PreparedRedirectScheme::Https));
        assert_eq!(prepared.port().map(NonZeroU16::get), Some(8443));
        assert_eq!(prepared.status_code(), StatusCode::TEMPORARY_REDIRECT);

        let invalid: HTTPURLRewriteFilter = serde_json::from_value(json!({
            "path": {"type": "ReplaceFullPath", "replaceFullPath": "/x?leak=1"}
        }))
        .unwrap();
        assert!(invalid.prepare().is_err());

        for replacement in ["relative", "/fragment#part"] {
            let invalid: HTTPURLRewriteFilter = serde_json::from_value(json!({
                "path": {"type": "ReplaceFullPath", "replaceFullPath": replacement}
            }))
            .unwrap();
            assert!(invalid.prepare().is_err());
        }
    }

    #[test]
    fn empty_full_path_normalizes_and_empty_prefix_is_valid() {
        let full: HTTPURLRewriteFilter = serde_json::from_value(json!({
            "path": {"type": "ReplaceFullPath", "replaceFullPath": ""}
        }))
        .unwrap();
        assert_eq!(
            full.prepare().unwrap().path(),
            Some(&PreparedPathModifier::ReplaceFullPath("/".to_string()))
        );

        let prefix: HTTPURLRewriteFilter = serde_json::from_value(json!({
            "path": {"type": "ReplacePrefixMatch", "replacePrefixMatch": ""}
        }))
        .unwrap();
        assert_eq!(
            prefix.prepare().unwrap().path(),
            Some(&PreparedPathModifier::ReplacePrefixMatch(String::new()))
        );
    }

    #[test]
    fn path_modifier_nulls_remain_invalid() {
        assert!(serde_json::from_value::<HTTPURLRewriteFilter>(json!({
            "path": {"type": null, "replaceFullPath": "/v2"}
        }))
        .is_err());

        let missing_replacement: HTTPURLRewriteFilter = serde_json::from_value(json!({
            "path": {"type": "ReplaceFullPath", "replaceFullPath": null}
        }))
        .unwrap();
        assert!(missing_replacement.prepare().is_err());
    }

    #[test]
    fn typed_path_match_defaults_come_from_the_contract() {
        let explicit_empty: HTTPRouteMatch = serde_json::from_value(json!({"path": {}})).unwrap();
        let path = explicit_empty.path.unwrap();
        assert_eq!(path.match_type.as_deref(), Some("PathPrefix"));
        assert_eq!(path.value.as_deref(), Some("/"));

        let omitted: HTTPRouteMatch = serde_json::from_value(json!({})).unwrap();
        assert_eq!(omitted.path.unwrap().value.as_deref(), Some("/"));

        for (input, expected_type, expected_value) in [
            (json!({"path": null}), "PathPrefix", "/"),
            (json!({"path": {"type": null, "value": "/api"}}), "PathPrefix", "/api"),
            (json!({"path": {"type": "Exact", "value": null}}), "Exact", "/"),
        ] {
            let json_match: HTTPRouteMatch = serde_json::from_value(input.clone()).unwrap();
            let yaml_match: HTTPRouteMatch = serde_yaml::from_str(&serde_yaml::to_string(&input).unwrap()).unwrap();
            for parsed in [json_match, yaml_match] {
                let path = parsed.path.as_ref().unwrap();
                assert_eq!(path.match_type.as_deref(), Some(expected_type));
                assert_eq!(path.value.as_deref(), Some(expected_value));

                let encoded = serde_json::to_value(&parsed).unwrap();
                let round_trip: HTTPRouteMatch = serde_json::from_value(encoded.clone()).unwrap();
                assert_eq!(serde_json::to_value(round_trip).unwrap(), encoded);
            }
        }
    }

    #[test]
    fn edgion_plugin_uses_the_same_config_fragment() {
        let plugin = EdgionPlugin::RequestRedirect(serde_json::from_value(json!({"scheme": "ftp"})).unwrap());
        assert!(plugin.validation_error().is_some());
    }

    #[test]
    fn effective_safety_catches_multiple_backend_prefix_transforms() {
        let rule = rule(json!({
            "matches": [{"path": {"type": "Exact", "value": "/api"}}],
            "backendRefs": [
                {"name": "a", "filters": [{"type": "URLRewrite", "urlRewrite": {"path": {
                    "type": "ReplacePrefixMatch", "replacePrefixMatch": "/a"
                }}}]},
                {"name": "b", "filters": [{"type": "URLRewrite", "urlRewrite": {"path": {
                    "type": "ReplacePrefixMatch", "replacePrefixMatch": "/b"
                }}}]}
            ]
        }));
        for violations in [
            authored_request_transform_violations(&rule).unwrap(),
            effective_request_transform_violations(&rule).unwrap(),
        ] {
            assert!(violations
                .iter()
                .any(|violation| violation.message.contains("PathPrefix")));
        }
    }
}
