//! Declarative REST request adaptation through canonical BodyStorage and KeyGet.

use super::common::{graphql_request, json_request_headers_are_supported, supported_identity_encoding, CappedWriter};
use crate::plugins::runtime::body_gate::record_body_read_error_status;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::request_body::BodyReadError;
use async_trait::async_trait;
use bytes::Bytes;
use edgion_common::startup::catch_redacted_panic;
use edgion_resources::common::KeyGet;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::graphql_request::{GraphqlError, GraphqlOperationType, ParsedGraphqlRequest};
use edgion_resources::plugin_configs::{
    parse_json_instance_with_limits, validate_rest_query, CompiledRestToGraphql, RestToGraphqlConfig,
    REST_GRAPHQL_MAX_VALUE_BYTES,
};
use edgion_resources::resources::edgion_plugins::BodyRequirement;
use edgion_resources::EdgionErrorCode;
use serde::Serialize;
use serde_json::{Map, Value};

pub struct RestToGraphql {
    compiled: CompiledRestToGraphql,
    envelope_limit: usize,
}

impl RestToGraphql {
    pub fn create(config: &RestToGraphqlConfig, body: Option<BodyRequirement>) -> Option<Box<dyn RequestFilter>> {
        Some(Box::new(Self {
            compiled: catch_redacted_panic(|| config.compile()).ok()?.ok()?,
            envelope_limit: graphql_request::envelope_limit(body)?,
        }))
    }

    fn construct(&self, session: &dyn PluginSession, body: Option<&Value>) -> Result<Bytes, GraphqlError> {
        let mut variables = Map::new();
        for (index, binding) in self.compiled.bindings().enumerate() {
            let local;
            let value = if let Some(source) = &binding.from {
                if let KeyGet::Header { name } = source {
                    let mut values = session.request_header_map().get_all(name).iter();
                    if let Some(value) = values.next() {
                        if values.next().is_some() || value.to_str().is_err() {
                            return Err(GraphqlError::Invalid);
                        }
                        if value.as_bytes().len() > REST_GRAPHQL_MAX_VALUE_BYTES {
                            return Err(GraphqlError::TooLarge);
                        }
                    }
                }
                local = session.key_get(source).map(Value::String);
                local.as_ref()
            } else {
                body.and_then(|body| self.compiled.body_value(index, body))
            };
            if let Some(value) = self.compiled.bind(index, value)? {
                variables.insert(binding.name.clone(), value);
            }
        }
        #[derive(Serialize)]
        struct Envelope<'a> {
            query: &'a str,
            #[serde(rename = "operationName", skip_serializing_if = "Option::is_none")]
            operation_name: Option<&'a str>,
            variables: Map<String, Value>,
        }
        let mut writer = CappedWriter::new(self.envelope_limit);
        let result = serde_json::to_writer(
            &mut writer,
            &Envelope {
                query: &self.compiled.query,
                operation_name: self.compiled.operation_name.as_deref(),
                variables,
            },
        );
        if result.is_err() {
            return Err(if writer.overflowed() {
                GraphqlError::TooLarge
            } else {
                GraphqlError::Invalid
            });
        }
        let output = Bytes::from(writer.into_inner());
        // Verify the entire final envelope, including aggregate structural limits.
        ParsedGraphqlRequest::parse_post(&output, self.envelope_limit)?;
        Ok(output)
    }
}

#[async_trait]
impl RequestFilter for RestToGraphql {
    fn name(&self) -> &str {
        "RestToGraphql"
    }
    fn requires_materialized_request_body(&self) -> bool {
        false
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        let ctx = session.ctx();
        if !ctx.request_info.in_request_body_stage {
            log.push_err("invalid request stage");
            return graphql_request::reject(session, 500);
        }
        let Some(method) = session.get_request_method() else {
            log.push_err("missing client method");
            return graphql_request::reject(session, 500);
        };
        if !matches!(method, "GET" | "POST" | "PUT" | "PATCH" | "DELETE")
            || (method == "GET" && self.compiled.operation_type == GraphqlOperationType::Mutation)
        {
            log.push("deny method");
            return graphql_request::reject(session, 405);
        }
        if method == "GET" && ctx.request_info.body_declared != Some(false) {
            log.push("deny GET body");
            return graphql_request::reject(session, 400);
        }
        if graphql_request::unsupported_protocol(session) {
            log.push("deny protocol");
            return graphql_request::reject(session, 400);
        }
        if !supported_identity_encoding(session.request_header_map()) {
            log.push("deny encoding");
            return graphql_request::reject(session, 415);
        }
        if let Err(error) = validate_rest_query(
            session
                .ctx()
                .request_info
                .entry_raw_query
                .as_deref()
                .unwrap_or_default(),
            self.envelope_limit,
        ) {
            log.push("deny query parameters");
            return graphql_request::reject(session, if error == GraphqlError::TooLarge { 413 } else { 400 });
        }
        let known_bodyless = session.declares_body() == Some(false);
        let length = match session.request_body_available_len(known_bodyless) {
            Ok(length) => length,
            Err(error) => {
                record_body_read_error_status(session, &error);
                log.push_err("body length unavailable");
                return graphql_request::reject(session, 500);
            }
        };
        if length > self.envelope_limit as u64 {
            session.record_error(EdgionErrorCode::BodyReadTooLarge);
            log.push("deny input cap");
            return graphql_request::reject(session, 413);
        }
        if length != 0 && !json_request_headers_are_supported(session.request_header_map()) {
            log.push("deny media type");
            return graphql_request::reject(session, 415);
        }
        let output = {
            let Ok(_permit) = graphql_request::EVALUATIONS.try_acquire() else {
                log.push_err("query budget exhausted");
                return graphql_request::reject(session, 503);
            };
            let input = if length == 0 {
                Bytes::new()
            } else {
                match session.read_request_body(None).await {
                    Ok(input) => input,
                    Err(error) => {
                        record_body_read_error_status(session, &error);
                        log.push_err("body unavailable");
                        return graphql_request::reject(session, 500);
                    }
                }
            };
            catch_redacted_panic(|| {
                if input.len() > self.envelope_limit {
                    return Err(GraphqlError::TooLarge);
                }
                let body = if input.is_empty() {
                    None
                } else {
                    Some(
                        parse_json_instance_with_limits(&input, 32, 4096)
                            .map_err(|_| GraphqlError::Invalid)?
                            .into_value(),
                    )
                };
                self.construct(session, body.as_ref())
            })
        };
        let output = match output {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => {
                log.push("deny variable mapping");
                if error == GraphqlError::TooLarge {
                    session.record_error(EdgionErrorCode::BodyReadTooLarge);
                }
                return graphql_request::reject(session, if error == GraphqlError::TooLarge { 413 } else { 400 });
            }
            Err(_) => {
                log.push_err("adapter evaluation panic");
                return graphql_request::reject(session, 500);
            }
        };
        if let Err(error) = session.overwrite_request_body(output).await {
            let status = if matches!(
                error,
                BodyReadError::TooLarge { .. } | BodyReadError::BufferLimitExceeded { .. }
            ) {
                413
            } else {
                500
            };
            record_body_read_error_status(session, &error);
            log.push_err("body publication failed");
            return graphql_request::reject(session, status);
        }
        if session.set_upstream_method("POST").is_err()
            || session.set_upstream_uri(&self.compiled.path).is_err()
            || session.set_request_header("content-type", "application/json").is_err()
        {
            log.push_err("request publication failed");
            return graphql_request::reject(session, 500);
        }
        log.push("request adapted");
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/rest_to_graphql/mod_test.rs"]
mod tests;
