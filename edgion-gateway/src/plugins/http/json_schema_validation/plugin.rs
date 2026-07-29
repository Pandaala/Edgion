use std::panic::{catch_unwind, AssertUnwindSafe};

use async_trait::async_trait;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{
    parse_json_instance, CompiledJsonSchema, InstanceValidationError, JsonSchemaValidationConfig, StrictJsonError,
    ABSOLUTE_INSTANCE_MAX_BYTES,
};
use edgion_resources::EdgionStatus;

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
        };
        if let Some(status) = status {
            session.add_error(status);
        }
    }

    fn header_values_are_supported(content_types: Option<&[String]>, content_encodings: Option<&[String]>) -> bool {
        let Some(content_types) = content_types else {
            return false;
        };
        if content_types.len() != 1 {
            return false;
        }

        let Ok(content_type) = content_types[0].trim().parse::<mime::Mime>() else {
            return false;
        };
        let subtype = content_type.subtype();
        let is_json = subtype == mime::JSON || (subtype.as_str() != "*" && content_type.suffix() == Some(mime::JSON));
        if content_type.type_() != mime::APPLICATION || !is_json {
            return false;
        }

        let mut charset_count = 0usize;
        for (name, value) in content_type.params() {
            if name == mime::CHARSET {
                charset_count += 1;
                if charset_count > 1 || !value.as_str().eq_ignore_ascii_case("utf-8") {
                    return false;
                }
            }
        }

        let Some(content_encodings) = content_encodings else {
            return false;
        };
        match content_encodings {
            [] => true,
            [value] => value.trim().eq_ignore_ascii_case("identity"),
            _ => false,
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
        if !Self::header_values_are_supported(content_types.as_deref(), content_encodings.as_deref()) {
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
        let mut session = MockPluginSession::new();
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

    fn headers_are_supported(content_types: &[&str], content_encodings: &[&str]) -> bool {
        let content_types: Vec<String> = content_types.iter().map(|value| (*value).to_string()).collect();
        let content_encodings: Vec<String> = content_encodings.iter().map(|value| (*value).to_string()).collect();
        JsonSchemaValidation::header_values_are_supported(Some(&content_types), Some(&content_encodings))
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

    #[test]
    fn media_type_matrix_is_strict() {
        for accepted in [
            "application/json",
            "APPLICATION/JSON",
            " application/json \t",
            "application/problem+json",
            "application/problem+json; charset=utf-8; version=1",
        ] {
            assert!(headers_are_supported(&[accepted], &[]), "{accepted}");
        }

        for rejected in [
            "text/json",
            "text/problem+json",
            "application/*+json",
            "application/json; charset=iso-8859-1",
            "application/json, text/plain",
        ] {
            assert!(!headers_are_supported(&[rejected], &[]), "{rejected}");
        }

        assert!(!headers_are_supported(&[], &[]));
        assert!(!headers_are_supported(&["application/json", "application/json"], &[]));
        assert!(!JsonSchemaValidation::header_values_are_supported(None, Some(&[])));
    }

    #[test]
    fn content_encoding_allows_only_absent_or_single_identity() {
        assert!(headers_are_supported(&["application/json"], &[]));
        assert!(headers_are_supported(&["application/json"], &["identity"]));
        assert!(!headers_are_supported(&["application/json"], &["gzip"]));
        assert!(!headers_are_supported(&["application/json"], &["identity, identity"]));
        assert!(!JsonSchemaValidation::header_values_are_supported(
            Some(&["application/json".to_string()]),
            None
        ));
    }

    #[tokio::test]
    async fn media_type_rejection_precedes_materialization() {
        let filter = plugin(json!({}));
        let body = br#"{"name":"Ada"}"#;
        let mut session = MockPluginSession::new();
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
}
