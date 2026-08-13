use std::panic::{catch_unwind, AssertUnwindSafe};

use async_trait::async_trait;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{
    parse_json_instance, CompiledJsonSchema, InstanceValidationError, JsonSchemaValidationConfig, StrictJsonError,
    ABSOLUTE_INSTANCE_MAX_BYTES,
};
use edgion_resources::EdgionStatus;

use crate::plugins::http::common::json_request_headers_are_supported;
use crate::plugins::runtime::traits::session::BodyReadError;
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
            body: body.map(str::to_owned),
        }
    }

    fn record_body_error(session: &mut dyn PluginSession, error: &BodyReadError) {
        let status = match error {
            BodyReadError::BufferLimitExceeded { .. } | BodyReadError::TooLarge { .. } => {
                Some(EdgionStatus::BodyReadTooLarge)
            }
            BodyReadError::Timeout => Some(EdgionStatus::BodyReadTimeout),
            BodyReadError::Incomplete => Some(EdgionStatus::BodyReadAborted),
            BodyReadError::Unimplemented => Some(EdgionStatus::BodyReadUnimplemented),
            // The adapter records the more precise refusal status.
            BodyReadError::NotBuffered => None,
            BodyReadError::StorageFailure => Some(EdgionStatus::BodyCaptureStorageFailed),
            BodyReadError::GenerationConflict => Some(EdgionStatus::BodyMutationConflict),
        };
        if let Some(status) = status {
            session.add_error(status);
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
                Self::record_body_error(session, &error);
                log.push_err("body length unavailable");
                return Self::reject(500, None);
            }
        };

        if body_len == 0 {
            log.push("deny empty body");
            return Self::reject(400, Some(INVALID_REQUEST_BODY));
        }
        if body_len > ABSOLUTE_INSTANCE_MAX_BYTES {
            session.add_error(EdgionStatus::BodyReadTooLarge);
            log.push("deny body too large");
            return Self::reject(413, None);
        }
        let content_types = session.request_header_values("content-type");
        let content_encodings = session.request_header_values("content-encoding");
        if !json_request_headers_are_supported(content_types.as_deref(), content_encodings.as_deref()) {
            log.push("deny media type");
            return Self::reject(415, Some(UNSUPPORTED_MEDIA_TYPE));
        }

        let body = match session.read_request_body(None).await {
            Ok(body) => body,
            Err(error) => {
                Self::record_body_error(session, &error);
                log.push_err("body materialization failed");
                return Self::reject(500, None);
            }
        };
        if body.len() as u64 > ABSOLUTE_INSTANCE_MAX_BYTES {
            session.add_error(EdgionStatus::BodyReadTooLarge);
            log.push("deny body too large");
            return Self::reject(413, None);
        }

        let parsed = match parse_json_instance(&body) {
            Ok(parsed) => parsed,
            Err(StrictJsonError::TooLarge) => {
                session.add_error(EdgionStatus::BodyReadTooLarge);
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
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use bytes::Bytes;
    use serde_json::json;

    fn plugin(schema: serde_json::Value) -> Box<dyn RequestFilter> {
        JsonSchemaValidation::create(&JsonSchemaValidationConfig { schema }).expect("schema compiles")
    }

    #[test]
    fn invalid_schema_constructor_declines_runtime_publication() {
        assert!(JsonSchemaValidation::create(&JsonSchemaValidationConfig {
            schema: json!({"$schema": "http://json-schema.org/draft-07/schema#"})
        })
        .is_none());
    }

    fn session(body: &'static [u8], content_type: &str) -> MockPluginSession {
        session_with_claims(body, content_type, vec![true])
    }

    fn session_with_claims(body: &'static [u8], content_type: &str, claims: Vec<bool>) -> MockPluginSession {
        let mut session = MockPluginSession::new();
        let claim_count = claims.len();
        let mut claims = claims.into_iter();
        session
            .expect_claim_json_schema_validation()
            .times(claim_count)
            .returning(move || claims.next().expect("claim expectation exhausted"));
        session.expect_declares_body().return_const(Some(!body.is_empty()));
        session
            .expect_request_body_available_len()
            .with(mockall::predicate::eq(false))
            .return_const(Ok(body.len() as u64));
        let content_type = content_type.to_string();
        session
            .expect_request_header_values()
            .times(2)
            .returning(move |name| match name {
                "content-type" => Some(vec![content_type.clone()]),
                "content-encoding" => Some(Vec::new()),
                _ => unreachable!("unexpected header name"),
            });
        session
            .expect_read_request_body()
            .with(mockall::predicate::eq(None))
            .return_once(move |_| Ok(Bytes::from_static(body)));
        session
    }

    #[tokio::test]
    async fn valid_json_passes_without_gate_materialization() {
        let filter = plugin(json!({
            "type": "object",
            "required": ["name"],
            "properties": {"name": {"type": "string"}}
        }));
        assert!(!filter.requires_materialized_request_body());
        let mut session = session(br#"{"name":"Ada"}"#, "application/json; charset=UTF-8");
        let mut log = PluginLog::new(filter.name());

        assert_eq!(
            filter.run_request(&mut session, &mut log).await,
            PluginRunningResult::Nothing
        );
        assert!(log.contains("pass"));
    }

    #[tokio::test]
    async fn schema_mismatch_and_malformed_json_share_generic_400() {
        let filter = plugin(json!({"type": "object", "required": ["name"]}));
        for body in [br#"{"other":1}"#.as_slice(), br#"{"name":"#.as_slice()] {
            let mut session = session(body, "application/json");
            let mut log = PluginLog::new(filter.name());
            assert_eq!(
                filter.run_request(&mut session, &mut log).await,
                JsonSchemaValidation::reject(400, Some(INVALID_REQUEST_BODY))
            );
        }
    }

    #[tokio::test]
    async fn empty_body_wins_over_missing_content_type_without_materializing() {
        let filter = plugin(json!({}));
        let mut session = MockPluginSession::new();
        session.expect_claim_json_schema_validation().return_const(true);
        session.expect_declares_body().return_const(Some(false));
        session
            .expect_request_body_available_len()
            .with(mockall::predicate::eq(true))
            .return_const(Ok(0));
        let mut log = PluginLog::new(filter.name());

        assert_eq!(
            filter.run_request(&mut session, &mut log).await,
            JsonSchemaValidation::reject(400, Some(INVALID_REQUEST_BODY))
        );
    }

    #[tokio::test]
    async fn absolute_limit_wins_over_media_type_without_materializing() {
        let filter = plugin(json!({}));
        let mut session = MockPluginSession::new();
        session.expect_claim_json_schema_validation().return_const(true);
        session.expect_declares_body().return_const(Some(true));
        session
            .expect_request_body_available_len()
            .with(mockall::predicate::eq(false))
            .return_const(Ok(ABSOLUTE_INSTANCE_MAX_BYTES + 1));
        session
            .expect_add_error()
            .with(mockall::predicate::eq(EdgionStatus::BodyReadTooLarge))
            .return_once(|_| ());
        let mut log = PluginLog::new(filter.name());

        assert_eq!(
            filter.run_request(&mut session, &mut log).await,
            JsonSchemaValidation::reject(413, None)
        );
    }

    #[tokio::test]
    async fn media_type_rejection_precedes_materialization() {
        let filter = plugin(json!({}));
        let body = br#"{"name":"Ada"}"#;
        let mut session = MockPluginSession::new();
        session.expect_claim_json_schema_validation().return_const(true);
        session.expect_declares_body().return_const(Some(true));
        session
            .expect_request_body_available_len()
            .with(mockall::predicate::eq(false))
            .return_const(Ok(body.len() as u64));
        session
            .expect_request_header_values()
            .times(2)
            .returning(|_| Some(Vec::new()));
        let mut log = PluginLog::new(filter.name());

        assert_eq!(
            filter.run_request(&mut session, &mut log).await,
            JsonSchemaValidation::reject(415, Some(UNSUPPORTED_MEDIA_TYPE))
        );
    }

    #[tokio::test]
    async fn materialization_failure_is_fixed_500() {
        let filter = plugin(json!({}));
        let body = br#"{"name":"Ada"}"#;
        let mut session = MockPluginSession::new();
        session.expect_claim_json_schema_validation().return_const(true);
        session.expect_declares_body().return_const(Some(true));
        session
            .expect_request_body_available_len()
            .with(mockall::predicate::eq(false))
            .return_const(Ok(body.len() as u64));
        session
            .expect_request_header_values()
            .times(2)
            .returning(|name| match name {
                "content-type" => Some(vec!["application/json".to_string()]),
                "content-encoding" => Some(Vec::new()),
                _ => unreachable!("unexpected header name"),
            });
        session
            .expect_read_request_body()
            .with(mockall::predicate::eq(None))
            .return_once(|_| Err(BodyReadError::StorageFailure));
        session
            .expect_add_error()
            .with(mockall::predicate::eq(EdgionStatus::BodyCaptureStorageFailed))
            .return_once(|_| ());
        let mut log = PluginLog::new(filter.name());

        assert_eq!(
            filter.run_request(&mut session, &mut log).await,
            JsonSchemaValidation::reject(500, None)
        );
        assert!(log.contains("err: body materialization failed"));
    }

    #[tokio::test]
    async fn second_validator_fails_closed_before_body_access() {
        let filter = plugin(json!({"type": "object"}));
        let mut session = session_with_claims(br#"{"name":"Ada"}"#, "application/json", vec![true, false]);

        let mut first_log = PluginLog::new(filter.name());
        assert_eq!(
            filter.run_request(&mut session, &mut first_log).await,
            PluginRunningResult::Nothing
        );

        let mut second_log = PluginLog::new(filter.name());
        assert_eq!(
            filter.run_request(&mut session, &mut second_log).await,
            JsonSchemaValidation::reject(500, None)
        );
        assert!(second_log.contains("err: multiple validators in effective request chain"));
    }

    #[tokio::test]
    async fn conditionally_skipped_validator_does_not_claim_request_slot() {
        use crate::plugins::runtime::conditional_filter::ConditionalRequestFilter;
        use edgion_resources::common::KeyGet;
        use edgion_resources::resources::edgion_plugins::conditions::{Condition, IncludeCondition, PluginConditions};

        let skipped = ConditionalRequestFilter::new(
            plugin(json!({"type": "object"})),
            Some(PluginConditions {
                skip: None,
                run: Some(vec![Condition::Include(IncludeCondition {
                    key: KeyGet::Method,
                    values: Some(vec!["POST".to_string()]),
                    regex: None,
                    values_set: None,
                    wildcard_patterns: None,
                    compiled_regex: None,
                })]),
            }),
        );
        let active = plugin(json!({"type": "object"}));
        let mut session = session(br#"{"name":"Ada"}"#, "application/json");
        session
            .expect_key_get()
            .with(mockall::predicate::eq(KeyGet::Method))
            .return_const(Some("GET".to_string()));

        let mut skipped_log = PluginLog::new(skipped.name());
        assert_eq!(
            skipped.run_request(&mut session, &mut skipped_log).await,
            PluginRunningResult::Nothing
        );
        assert!(skipped_log.is_cond_skipped());

        let mut active_log = PluginLog::new(active.name());
        assert_eq!(
            active.run_request(&mut session, &mut active_log).await,
            PluginRunningResult::Nothing
        );
    }
}
