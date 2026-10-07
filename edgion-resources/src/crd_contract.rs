//! Shared validation for selected, generated Kubernetes CRD contracts.
//!
//! Contracts are generated from vendored CRDs and embedded as compact JSON.
//! Validation runs only while configuration is prepared; request processing
//! never parses schemas or evaluates CEL.

use cel::{to_value, Context, Program, Value as CelValue};
use jsonschema::Validator;
use serde::Deserialize;
use serde_json::Value;
use std::sync::LazyLock;

const MAX_INPUT_BYTES: usize = 256 * 1024;
const MAX_INPUT_DEPTH: usize = 64;
const MAX_VIOLATIONS: usize = 32;
const HTTP_ROUTE_TRANSFORM_CONTRACT_JSON: &str = include_str!("contracts/http_route_transform_v1_6.json");

/// A stable validation failure that never embeds the submitted value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrdViolation {
    pub instance_path: String,
    pub schema_path: String,
    pub message: String,
}

/// Failure to load or execute an embedded validation contract.
#[derive(Debug, thiserror::Error)]
pub enum CrdContractError {
    #[error("embedded CRD contract is invalid")]
    InvalidContract,
    #[error("CRD validation input exceeds the configured bound")]
    InputLimit,
    #[error("embedded CEL validation could not be evaluated")]
    CelEvaluation,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContractSource {
    group: String,
    version: String,
    kind: String,
    channel: String,
    gateway_api_version: String,
    selected_sha256: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContractDefaults {
    matches: Value,
    path: Value,
    path_type: String,
    path_value: String,
    redirect_status_code: i32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContractFragments {
    filter_list: Value,
    request_redirect: Value,
    url_rewrite: Value,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContractRules {
    filter: Vec<CelRuleSource>,
    filter_list: Vec<CelRuleSource>,
    rule: Vec<CelRuleSource>,
}

#[derive(Clone, Debug, Deserialize)]
struct CelRuleSource {
    message: String,
    rule: String,
}

#[derive(Clone, Debug, Deserialize)]
struct ContractDocument {
    source: ContractSource,
    defaults: ContractDefaults,
    fragments: ContractFragments,
    rules: ContractRules,
}

#[derive(Clone, Debug)]
enum InstanceStep {
    Property(String),
    Items,
}

struct CompiledCelRule {
    program: Program,
    message: String,
    schema_path: String,
}

struct ScopedCelRules {
    instance_steps: Vec<InstanceStep>,
    rules: Vec<CompiledCelRule>,
}

pub(crate) struct CompiledFragment {
    schema: Value,
    validator: Validator,
    cel_rules: Vec<ScopedCelRules>,
}

impl CompiledFragment {
    pub(crate) fn compile(schema: Value, name: &str) -> Result<Self, CrdContractError> {
        let validator = Validator::new(&schema).map_err(|_| CrdContractError::InvalidContract)?;
        let mut cel_rules = Vec::new();
        collect_scoped_cel(&schema, &mut Vec::new(), name, &mut cel_rules)?;
        Ok(Self {
            schema,
            validator,
            cel_rules,
        })
    }

    fn validate(&self, input: &Value) -> Result<Vec<CrdViolation>, CrdContractError> {
        self.prepare(input).map(|(_, violations)| violations)
    }

    /// Normalize only the supplied bounded projection, keeping the authored view separate.
    pub(crate) fn prepare(&self, input: &Value) -> Result<(Value, Vec<CrdViolation>), CrdContractError> {
        check_input_bounds(input)?;
        let mut normalized = input.clone();
        prune_non_nullable_null_properties(&self.schema, &mut normalized);
        apply_defaults(&self.schema, &mut normalized);

        let mut violations = Vec::new();
        for error in self.validator.iter_errors(&normalized).take(MAX_VIOLATIONS) {
            violations.push(CrdViolation {
                instance_path: error.instance_path().to_string(),
                schema_path: error.schema_path().to_string(),
                message: "value does not satisfy the vendored Gateway API schema".to_string(),
            });
        }
        if violations.is_empty() {
            evaluate_scoped_rules(&normalized, &self.cel_rules, &mut violations)?;
        }
        Ok((normalized, violations))
    }
}

struct CompiledRuleSet {
    rules: Vec<CompiledCelRule>,
}

impl CompiledRuleSet {
    fn compile(sources: Vec<CelRuleSource>, schema_path: &str) -> Result<Self, CrdContractError> {
        let rules = sources
            .into_iter()
            .enumerate()
            .map(|(index, source)| {
                Ok(CompiledCelRule {
                    program: Program::compile(&source.rule).map_err(|_| CrdContractError::InvalidContract)?,
                    message: source.message,
                    schema_path: format!("{schema_path}/x-kubernetes-validations/{index}"),
                })
            })
            .collect::<Result<Vec<_>, CrdContractError>>()?;
        Ok(Self { rules })
    }

    fn validate(&self, input: &Value) -> Result<Vec<CrdViolation>, CrdContractError> {
        check_input_bounds(input)?;
        let mut violations = Vec::new();
        evaluate_rules(input, "", &self.rules, &mut violations)?;
        Ok(violations)
    }
}

/// Generated defaults shared by validation and typed runtime preparation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpRouteTransformDefaults {
    pub matches: Value,
    pub path: Value,
    pub path_type: String,
    pub path_value: String,
    pub redirect_status_code: i32,
}

/// Compiled request-transform subset of the vendored HTTPRoute/v1 CRD.
pub struct HttpRouteTransformContract {
    request_redirect: CompiledFragment,
    url_rewrite: CompiledFragment,
    filter_list_bounds: CompiledFragment,
    filter: CompiledRuleSet,
    filter_list: CompiledRuleSet,
    rule: CompiledRuleSet,
    defaults: HttpRouteTransformDefaults,
}

impl HttpRouteTransformContract {
    fn compile() -> Result<Self, CrdContractError> {
        let document: ContractDocument =
            serde_json::from_str(HTTP_ROUTE_TRANSFORM_CONTRACT_JSON).map_err(|_| CrdContractError::InvalidContract)?;
        if document.source.group != "gateway.networking.k8s.io"
            || document.source.version != "v1"
            || document.source.kind != "HTTPRoute"
            || document.source.channel != "standard"
            || document.source.gateway_api_version != "v1.6.2"
            || document.source.selected_sha256.len() != 64
        {
            return Err(CrdContractError::InvalidContract);
        }
        Ok(Self {
            request_redirect: CompiledFragment::compile(
                document.fragments.request_redirect,
                "/fragments/requestRedirect",
            )?,
            url_rewrite: CompiledFragment::compile(document.fragments.url_rewrite, "/fragments/urlRewrite")?,
            filter_list_bounds: CompiledFragment::compile(document.fragments.filter_list, "/fragments/filterList")?,
            filter: CompiledRuleSet::compile(document.rules.filter, "/rules/filter")?,
            filter_list: CompiledRuleSet::compile(document.rules.filter_list, "/rules/filterList")?,
            rule: CompiledRuleSet::compile(document.rules.rule, "/rules/rule")?,
            defaults: HttpRouteTransformDefaults {
                matches: document.defaults.matches,
                path: document.defaults.path,
                path_type: document.defaults.path_type,
                path_value: document.defaults.path_value,
                redirect_status_code: document.defaults.redirect_status_code,
            },
        })
    }

    pub fn defaults(&self) -> &HttpRouteTransformDefaults {
        &self.defaults
    }

    pub fn validate_request_redirect(&self, value: &Value) -> Result<Vec<CrdViolation>, CrdContractError> {
        self.request_redirect.validate(value)
    }

    pub fn validate_url_rewrite(&self, value: &Value) -> Result<Vec<CrdViolation>, CrdContractError> {
        self.url_rewrite.validate(value)
    }

    pub fn validate_filter(&self, value: &Value) -> Result<Vec<CrdViolation>, CrdContractError> {
        self.filter.validate(value)
    }

    pub fn validate_filter_list(&self, value: &Value) -> Result<Vec<CrdViolation>, CrdContractError> {
        self.filter_list.validate(value)
    }

    pub fn validate_filter_list_bounds(&self, value: &Value) -> Result<Vec<CrdViolation>, CrdContractError> {
        self.filter_list_bounds.validate(value)
    }

    pub fn validate_rule(&self, value: &Value) -> Result<Vec<CrdViolation>, CrdContractError> {
        self.rule.validate(value)
    }
}

static HTTP_ROUTE_TRANSFORM_CONTRACT: LazyLock<Result<HttpRouteTransformContract, CrdContractError>> =
    LazyLock::new(HttpRouteTransformContract::compile);

pub fn http_route_transform_contract() -> Result<&'static HttpRouteTransformContract, CrdContractError> {
    HTTP_ROUTE_TRANSFORM_CONTRACT
        .as_ref()
        .map_err(|_| CrdContractError::InvalidContract)
}

fn collect_scoped_cel(
    schema: &Value,
    steps: &mut Vec<InstanceStep>,
    schema_path: &str,
    output: &mut Vec<ScopedCelRules>,
) -> Result<(), CrdContractError> {
    if let Some(sources) = schema.get("x-kubernetes-validations").and_then(Value::as_array) {
        let rules = sources
            .iter()
            .enumerate()
            .map(|(index, source)| {
                let message = source
                    .get("message")
                    .and_then(Value::as_str)
                    .ok_or(CrdContractError::InvalidContract)?;
                let rule = source
                    .get("rule")
                    .and_then(Value::as_str)
                    .ok_or(CrdContractError::InvalidContract)?;
                Ok(CompiledCelRule {
                    program: Program::compile(rule).map_err(|_| CrdContractError::InvalidContract)?,
                    message: message.to_string(),
                    schema_path: format!("{schema_path}/x-kubernetes-validations/{index}"),
                })
            })
            .collect::<Result<Vec<_>, CrdContractError>>()?;
        output.push(ScopedCelRules {
            instance_steps: steps.clone(),
            rules,
        });
    }

    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        for (name, child) in properties {
            steps.push(InstanceStep::Property(name.clone()));
            collect_scoped_cel(child, steps, &format!("{schema_path}/properties/{name}"), output)?;
            steps.pop();
        }
    }
    if let Some(items) = schema.get("items") {
        steps.push(InstanceStep::Items);
        collect_scoped_cel(items, steps, &format!("{schema_path}/items"), output)?;
        steps.pop();
    }
    Ok(())
}

fn apply_defaults(schema: &Value, instance: &mut Value) {
    if instance.is_null() {
        return;
    }
    if let (Some(properties), Some(object)) = (
        schema.get("properties").and_then(Value::as_object),
        instance.as_object_mut(),
    ) {
        for (name, property_schema) in properties {
            if !object.contains_key(name) {
                if let Some(default) = property_schema.get("default") {
                    object.insert(name.clone(), default.clone());
                }
            }
            if let Some(value) = object.get_mut(name) {
                apply_defaults(property_schema, value);
            }
        }
    }
    if let (Some(item_schema), Some(items)) = (schema.get("items"), instance.as_array_mut()) {
        for item in items {
            apply_defaults(item_schema, item);
        }
    }
}

/// Mirror Kubernetes structural-schema handling for object fields: a null
/// value on a non-nullable property is pruned before defaults are applied.
fn prune_non_nullable_null_properties(schema: &Value, instance: &mut Value) {
    if let (Some(properties), Some(object)) = (
        schema.get("properties").and_then(Value::as_object),
        instance.as_object_mut(),
    ) {
        for (name, property_schema) in properties {
            if object.get(name).is_some_and(Value::is_null) && !schema_allows_null(property_schema) {
                object.remove(name);
                continue;
            }
            if let Some(value) = object.get_mut(name) {
                prune_non_nullable_null_properties(property_schema, value);
            }
        }
    }
    if let (Some(item_schema), Some(items)) = (schema.get("items"), instance.as_array_mut()) {
        for item in items {
            prune_non_nullable_null_properties(item_schema, item);
        }
    }
}

fn schema_allows_null(schema: &Value) -> bool {
    schema.get("nullable").and_then(Value::as_bool) == Some(true)
        || schema.get("type").is_some_and(|schema_type| match schema_type {
            Value::String(value) => value == "null",
            Value::Array(values) => values.iter().any(|value| value.as_str() == Some("null")),
            _ => false,
        })
}

fn evaluate_scoped_rules(
    root: &Value,
    scoped_rules: &[ScopedCelRules],
    violations: &mut Vec<CrdViolation>,
) -> Result<(), CrdContractError> {
    for scoped in scoped_rules {
        let mut values = vec![(root, String::new())];
        for step in &scoped.instance_steps {
            let mut next = Vec::new();
            for (value, path) in values {
                match step {
                    InstanceStep::Property(name) => {
                        if let Some(child) = value.get(name) {
                            next.push((child, format!("{path}/{name}")));
                        }
                    }
                    InstanceStep::Items => {
                        if let Some(items) = value.as_array() {
                            next.extend(
                                items
                                    .iter()
                                    .enumerate()
                                    .map(|(index, child)| (child, format!("{path}/{index}"))),
                            );
                        }
                    }
                }
            }
            values = next;
        }
        for (value, path) in values {
            evaluate_rules(value, &path, &scoped.rules, violations)?;
            if violations.len() >= MAX_VIOLATIONS {
                return Ok(());
            }
        }
    }
    Ok(())
}

fn evaluate_rules(
    value: &Value,
    instance_path: &str,
    rules: &[CompiledCelRule],
    violations: &mut Vec<CrdViolation>,
) -> Result<(), CrdContractError> {
    // Kubernetes CEL escapes reserved property names. This separate view does
    // not rename authored properties or affect OpenAPI validation.
    let cel_view = escape_cel_namespace(value);
    let cel_value = to_value(&cel_view).map_err(|_| CrdContractError::CelEvaluation)?;
    for rule in rules {
        let mut context = Context::default();
        context.add_variable_from_value("self", cel_value.clone());
        match rule
            .program
            .execute(&context)
            .map_err(|_| CrdContractError::CelEvaluation)?
        {
            CelValue::Bool(true) => {}
            CelValue::Bool(false) => violations.push(CrdViolation {
                instance_path: instance_path.to_string(),
                schema_path: rule.schema_path.clone(),
                message: rule.message.clone(),
            }),
            _ => return Err(CrdContractError::CelEvaluation),
        }
        if violations.len() >= MAX_VIOLATIONS {
            break;
        }
    }
    Ok(())
}

fn escape_cel_namespace(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, child)| {
                    (
                        if key == "namespace" {
                            "__namespace__".to_string()
                        } else {
                            key.clone()
                        },
                        escape_cel_namespace(child),
                    )
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(escape_cel_namespace).collect()),
        _ => value.clone(),
    }
}

pub(crate) fn check_input_bounds(value: &Value) -> Result<(), CrdContractError> {
    // Bound recursion before serialization or cloning any submitted subtree.
    fn within_depth(value: &Value, depth: usize) -> bool {
        if depth > MAX_INPUT_DEPTH {
            return false;
        }
        match value {
            Value::Array(values) => values.iter().all(|value| within_depth(value, depth + 1)),
            Value::Object(values) => values.values().all(|value| within_depth(value, depth + 1)),
            _ => true,
        }
    }
    if !within_depth(value, 1) {
        return Err(CrdContractError::InputLimit);
    }
    struct BoundedWriter {
        bytes: usize,
    }
    impl std::io::Write for BoundedWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes = self.bytes.saturating_add(bytes.len());
            if self.bytes > MAX_INPUT_BYTES {
                return Err(std::io::ErrorKind::InvalidData.into());
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(BoundedWriter { bytes: 0 }, value).map_err(|_| CrdContractError::InputLimit)
}

#[cfg(test)]
#[path = "../tests/unit/crd_contract_test.rs"]
mod tests;
