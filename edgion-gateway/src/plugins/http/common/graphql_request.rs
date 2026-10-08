//! Shared bounded GraphQL transport preparation and process-wide evaluation admission.

use super::{json_request_headers_are_supported, supported_identity_encoding};
use crate::plugins::runtime::body_gate::record_body_read_error_status;
use crate::plugins::runtime::{PluginLog, PluginSession};
use bytes::Bytes;
use edgion_common::startup::catch_redacted_panic;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::graphql_request::{GraphqlError, ParsedGraphqlRequest, MAX_ENVELOPE_BYTES};
use edgion_resources::resources::edgion_plugins::BodyRequirement;
use edgion_resources::EdgionErrorCode;
use std::sync::LazyLock;
use tokio::sync::Semaphore;

// Conservative source estimate: 96MiB per evaluation, not a measured heap cap.
// BodyStorage and immutable compiled generations have separate ownership.
pub(crate) const MAX_EVALUATIONS: usize = 4;
pub(crate) static EVALUATIONS: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(MAX_EVALUATIONS));
#[cfg(test)]
pub(crate) use tests::EVALUATION_TEST_LOCK;

pub(crate) fn envelope_limit(body: Option<BodyRequirement>) -> Option<usize> {
    let limit = match body {
        Some(body) => {
            if !edgion_resources::plugin_configs::graphql_body_validation_errors(&body).is_empty() {
                return None;
            }
            body.max_body_size_bytes()
                .transpose()
                .ok()?
                .unwrap_or(MAX_ENVELOPE_BYTES as u64)
        }
        None => MAX_ENVELOPE_BYTES as u64,
    };
    usize::try_from(limit).ok()
}

pub(crate) fn reject(status: u16) -> PluginRunningResult {
    PluginRunningResult::ErrResponse {
        status,
        body: Some(Bytes::from_static(b"GraphQL request rejected")),
    }
}

pub(crate) async fn evaluate<T, F>(
    session: &mut dyn PluginSession,
    log: &mut PluginLog,
    envelope_limit: usize,
    compute: F,
) -> Result<T, PluginRunningResult>
where
    F: FnOnce(ParsedGraphqlRequest) -> Result<T, GraphqlError> + Send,
{
    let ctx = session.ctx();
    if !ctx.request_info.in_request_body_stage {
        log.push_err("invalid request stage");
        return Err(reject(500));
    }
    if ctx.request_info.is_grpc_request
        || ctx.is_grpc_route_matched
        || matches!(
            ctx.request_info.discover_protocol.as_deref(),
            Some("grpc-web" | "websocket")
        )
        || ctx.has_request_body_stream_handlers()
        || ctx.grpc_transcoding.is_some()
        || session.request_header_map().contains_key(http::header::UPGRADE)
    {
        log.push("deny protocol");
        return Err(reject(400));
    }

    let is_post = match session.get_method() {
        "POST" => true,
        "GET" => false,
        _ => {
            log.push("deny method");
            return Err(reject(405));
        }
    };
    if is_post && session.request_query().is_some_and(|query| !query.is_empty()) {
        log.push("deny POST query params");
        return Err(reject(400));
    }
    let known_bodyless = session.declares_body() == Some(false);
    let body_len = match session.request_body_available_len(known_bodyless) {
        Ok(len) => len,
        Err(error) => {
            record_body_read_error_status(session, &error);
            log.push_err("body length unavailable");
            return Err(reject(500));
        }
    };
    if body_len > envelope_limit as u64
        || (!is_post
            && session
                .request_query()
                .is_some_and(|query| query.len() > envelope_limit))
    {
        session.record_error(EdgionErrorCode::BodyReadTooLarge);
        log.push("deny envelope cap");
        return Err(reject(413));
    }
    if !is_post && (!known_bodyless || body_len != 0) {
        log.push("deny GET body");
        return Err(reject(400));
    }
    if (is_post && !json_request_headers_are_supported(session.request_header_map()))
        || (!is_post && !supported_identity_encoding(session.request_header_map()))
    {
        log.push("deny media type");
        return Err(reject(415));
    }

    let Ok(_permit) = EVALUATIONS.try_acquire() else {
        log.push_err("query budget exhausted");
        return Err(reject(503));
    };
    let result = if is_post {
        let body = match session.read_request_body(None).await {
            Ok(body) => body,
            Err(error) => {
                record_body_read_error_status(session, &error);
                log.push_err("body materialization failed");
                return Err(reject(500));
            }
        };
        catch_redacted_panic(|| ParsedGraphqlRequest::parse_post(&body, envelope_limit).and_then(compute))
    } else {
        catch_redacted_panic(|| {
            ParsedGraphqlRequest::parse_get(session.request_query().unwrap_or_default(), envelope_limit)
                .and_then(compute)
        })
    };
    match result {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(GraphqlError::TooLarge)) => {
            session.record_error(EdgionErrorCode::BodyReadTooLarge);
            log.push("deny envelope cap");
            Err(reject(413))
        }
        Ok(Err(GraphqlError::MethodNotAllowed)) => {
            log.push("deny GET mutation");
            Err(reject(405))
        }
        Ok(Err(GraphqlError::Invalid)) => {
            log.push("deny invalid query");
            Err(reject(400))
        }
        Err(_) => {
            log.push_err("query evaluation panic");
            Err(reject(500))
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/common/graphql_request_test.rs"]
mod tests;
