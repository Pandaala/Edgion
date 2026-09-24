use bytes::Bytes;
use std::panic::{catch_unwind, AssertUnwindSafe};

use async_trait::async_trait;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{
    parse_json_instance, CompiledJsonSchema, InstanceValidationError, JsonSchemaValidationConfig, StrictJsonError,
    ABSOLUTE_INSTANCE_MAX_BYTES,
};
use edgion_resources::EdgionErrorCode;

use crate::plugins::http::common::json_request_headers_are_supported;
use crate::plugins::runtime::body_gate::record_body_read_error_status;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};

const INVALID_REQUEST_BODY: &str = "Invalid request body";
const UNSUPPORTED_MEDIA_TYPE: &str = "Unsupported media type";

/// Immutable request-stage JSON Schema validator.
pub struct JsonSchemaValidation {
    validator: CompiledJsonSchema,
}

impl JsonSchemaValidation {
    /// Compile the configured schema. A failure is returned as `None` so the
    /// runtime's standard constructor-failure seal rejects requests.
    pub fn create(config: &JsonSchemaValidationConfig) -> Option<Box<dyn RequestFilter>> {
        let compiled = catch_unwind(AssertUnwindSafe(|| config.compile())).ok()?.ok()?;
        Some(Box::new(Self { validator: compiled }))
    }

    fn reject(status: u16, body: Option<&'static str>) -> PluginRunningResult {
        PluginRunningResult::ErrResponse {
            status,
            body: body.map(|b| Bytes::from_static(b.as_bytes())),
        }
    }
}

#[async_trait]
impl RequestFilter for JsonSchemaValidation {
    fn name(&self) -> &str {
        "JsonSchemaValidation"
    }

    fn requires_materialized_request_body(&self) -> bool {
        false
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        if !session.claim_json_schema_validation() {
            log.push_err("multiple validators in effective request chain");
            return Self::reject(500, None);
        }

        let known_bodyless = session.declares_body() == Some(false);
        let body_len = match session.request_body_available_len(known_bodyless) {
            Ok(body_len) => body_len,
            Err(error) => {
                record_body_read_error_status(session, &error);
                log.push_err("body length unavailable");
                return Self::reject(500, None);
            }
        };

        if body_len == 0 {
            log.push("deny empty body");
            return Self::reject(400, Some(INVALID_REQUEST_BODY));
        }
        if body_len > ABSOLUTE_INSTANCE_MAX_BYTES {
            session.record_error(EdgionErrorCode::BodyReadTooLarge);
            log.push("deny body too large");
            return Self::reject(413, None);
        }
        if !json_request_headers_are_supported(session.request_header_map()) {
            log.push("deny media type");
            return Self::reject(415, Some(UNSUPPORTED_MEDIA_TYPE));
        }

        let body = match session.read_request_body(None).await {
            Ok(body) => body,
            Err(error) => {
                record_body_read_error_status(session, &error);
                log.push_err("body materialization failed");
                return Self::reject(500, None);
            }
        };
        if body.len() as u64 > ABSOLUTE_INSTANCE_MAX_BYTES {
            session.record_error(EdgionErrorCode::BodyReadTooLarge);
            log.push("deny body too large");
            return Self::reject(413, None);
        }

        let parsed = match parse_json_instance(&body) {
            Ok(parsed) => parsed,
            Err(StrictJsonError::TooLarge) => {
                session.record_error(EdgionErrorCode::BodyReadTooLarge);
                log.push("deny body too large");
                return Self::reject(413, None);
            }
            Err(_) => {
                log.push("deny malformed json");
                return Self::reject(400, Some(INVALID_REQUEST_BODY));
            }
        };

        let validation = catch_unwind(AssertUnwindSafe(|| self.validator.validate_instance(&parsed)));
        match validation {
            Ok(Ok(true)) => {
                log.push("pass");
                PluginRunningResult::Nothing
            }
            Ok(Ok(false)) => {
                log.push("deny schema mismatch");
                Self::reject(400, Some(INVALID_REQUEST_BODY))
            }
            Ok(Err(InstanceValidationError::WorkBudgetExceeded)) => {
                log.push("deny work budget");
                Self::reject(400, Some(INVALID_REQUEST_BODY))
            }
            Ok(Err(InstanceValidationError::ValidatorPanicked)) | Err(_) => {
                log.push_err("validator panic");
                Self::reject(500, None)
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/json_schema_validation/plugin_test.rs"]
mod tests;
