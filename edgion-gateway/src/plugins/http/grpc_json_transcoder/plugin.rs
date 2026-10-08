use super::{budget::Reservation, GrpcTranscodingPlan};
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use async_trait::async_trait;
use bytes::Bytes;
use edgion_resources::grpc_transcoding::GrpcDescriptorSource;
use edgion_resources::resources::{edgion_plugins::EdgionPlugin, refs::plugin_fields::BoundPluginEntry};
use edgion_resources::{
    filters::PluginRunningResult,
    grpc_transcoding::{CompiledGrpcTranscoder, GrpcJsonTranscoderConfig, TranscodingError, MAX_JSON_BYTES},
};
use std::sync::Arc;

pub struct GrpcJsonTranscoder {
    compiled: Arc<CompiledGrpcTranscoder>,
}
impl GrpcJsonTranscoder {
    /// The config and selector always come from the same actual owning entry.
    pub(crate) fn bound_config<'a>(entry: &'a BoundPluginEntry<'_>) -> Result<&'a GrpcJsonTranscoderConfig, String> {
        let EdgionPlugin::GrpcJsonTranscoder(config) = entry.config() else {
            return Err("invalid owning descriptor reference".into());
        };
        if !entry.is_enabled() {
            return Err("invalid owning descriptor reference".into());
        }
        if matches!(config.descriptor, GrpcDescriptorSource::ConfigMapRef { .. }) {
            if let Some(error) =
                edgion_resources::resources::edgion_plugins::validate::response_stage_plugin_validation_error(
                    entry.config(),
                )
            {
                return Err(error);
            }
            let references = entry.references().map_err(|_| "invalid owning descriptor reference")?;
            references
                .grpc_descriptor()
                .ok_or("invalid owning descriptor reference")?
                .map_err(|_| "invalid owning descriptor reference")?;
        }
        Ok(config)
    }

    pub(crate) fn create(entry: &BoundPluginEntry<'_>) -> Option<Box<dyn RequestFilter>> {
        Self::compile(Self::bound_config(entry).ok()?)
    }

    pub(crate) fn create_inline(config: &GrpcJsonTranscoderConfig) -> Option<Box<dyn RequestFilter>> {
        if !matches!(config.descriptor, GrpcDescriptorSource::Inline { .. }) {
            return None;
        }
        Self::compile(config)
    }

    fn compile(config: &GrpcJsonTranscoderConfig) -> Option<Box<dyn RequestFilter>> {
        let compiled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| config.compile()))
            .ok()?
            .ok()?;
        Some(Box::new(Self {
            compiled: Arc::new(compiled),
        }))
    }
}
pub(crate) async fn reject(session: &mut dyn PluginSession, status: u16) -> PluginRunningResult {
    let _ = session.set_response_header("content-type", "application/json");
    PluginRunningResult::ErrResponse {
        status,
        body: Some(Bytes::from_static(b"{\"error\":\"gRPC transcoding request failed\"}")),
    }
}
#[async_trait]
impl RequestFilter for GrpcJsonTranscoder {
    fn name(&self) -> &str {
        "GrpcJsonTranscoder"
    }
    fn requires_materialized_request_body(&self) -> bool {
        false
    }
    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        if !session.grpc_transcoding_allowed() {
            log.push_err("invalid conversion scope");
            return reject(session, 500).await;
        }
        let binding = match self
            .compiled
            .bind(session.get_method(), session.get_path(), session.get_query().as_deref())
        {
            Ok(binding) => binding,
            Err(error) => {
                log.push("deny mapping");
                return reject(session, error.request_status()).await;
            }
        };
        let Ok(len) = session.request_body_available_len(session.declares_body() == Some(false)) else {
            log.push_err("body unavailable");
            return reject(session, 500).await;
        };
        if len > MAX_JSON_BYTES as u64 {
            log.push("deny input limit");
            return reject(session, 413).await;
        }
        if len != 0 && !crate::plugins::http::common::json_request_headers_are_supported(session.request_header_map()) {
            log.push("deny media type");
            return reject(session, 415).await;
        }
        let Some(reservation) = Reservation::acquire() else {
            log.push_err("conversion budget exhausted");
            return reject(session, 503).await;
        };
        let body = if len == 0 {
            Bytes::new()
        } else {
            match session.read_request_body(None).await {
                Ok(body) if body.len() <= MAX_JSON_BYTES => body,
                _ => {
                    log.push_err("body read failed");
                    return reject(session, 500).await;
                }
            }
        };
        let encoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            binding.encode(&body, self.compiled.limits)
        }));
        let (binding, frame) = match encoded {
            Ok(Ok(encoded)) => encoded,
            Ok(Err(TranscodingError::TooLarge)) => {
                log.push("deny payload limit");
                return reject(session, 413).await;
            }
            _ => {
                log.push("deny invalid input");
                return reject(session, 400).await;
            }
        };
        let len = frame.len();
        if session.overwrite_request_body(Bytes::from(frame)).await.is_err()
            || session.set_upstream_method("POST").is_err()
            || session.set_upstream_uri(&binding.rpc_path).is_err()
        {
            log.push_err("request preparation failed");
            return reject(session, 500).await;
        }
        for name in ["content-encoding", "grpc-encoding", "grpc-timeout", "transfer-encoding"] {
            let _ = session.remove_request_header(name);
        }
        for (name, value) in [
            ("content-type", "application/grpc+proto"),
            ("te", "trailers"),
            ("grpc-accept-encoding", "identity"),
        ] {
            if session.set_request_header(name, value).is_err() {
                return reject(session, 500).await;
            }
        }
        let plan = GrpcTranscodingPlan::new(self.compiled.clone(), binding, reservation, len);
        if !session.install_grpc_transcoding(plan) {
            log.push_err("conversion owner conflict");
            return reject(session, 500).await;
        }
        // The production adapter installs the processor with the sealed owner.
        log.push("transcode");
        PluginRunningResult::Nothing
    }
}
