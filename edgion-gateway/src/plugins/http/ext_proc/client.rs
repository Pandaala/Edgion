//! gRPC client for the external processing service.
//!
//! The channel is built lazily at plugin construction (no blocking connect) and
//! multiplexes every request. Each processing phase opens its own short-lived
//! `Process` bidirectional stream on that shared channel (one stream per phase).

use std::time::Duration;

use edgion_common::ext_proc::proto::external_processor_client::ExternalProcessorClient;
use edgion_common::ext_proc::proto::{ProcessingRequest, ProcessingResponse};
use edgion_resources::plugin_configs::GrpcServiceConfig;
use tonic::transport::{Channel, ClientTlsConfig};

/// Thin wrapper around the generated ext_proc gRPC client.
#[derive(Clone)]
pub struct ExtProcClient {
    client: ExternalProcessorClient<Channel>,
    timeout: Duration,
}

impl ExtProcClient {
    /// Build a lazily-connecting client from the plugin's `grpcService` config.
    ///
    /// Returns `Err` only for a structurally invalid target; it never blocks on
    /// the network (the actual connection is established on first use). The
    /// config-layer `validation_error` already gates target/timeout, so a build
    /// failure here is unexpected and should fail the plugin closed.
    pub fn new(cfg: &GrpcServiceConfig) -> Result<Self, String> {
        let scheme = if cfg.tls { "https" } else { "http" };
        let uri = format!("{scheme}://{}", cfg.target);
        let timeout = Duration::from_millis(cfg.timeout_ms);

        let mut endpoint = Channel::from_shared(uri)
            .map_err(|e| format!("invalid ext_proc grpcService.target: {e}"))?
            .connect_timeout(timeout)
            .timeout(timeout);

        if cfg.tls {
            endpoint = endpoint
                .tls_config(ClientTlsConfig::new().with_webpki_roots())
                .map_err(|e| format!("ext_proc TLS config error: {e}"))?;
        }

        let channel = endpoint.connect_lazy();
        Ok(Self {
            client: ExternalProcessorClient::new(channel),
            timeout,
        })
    }

    /// Send one phase to the processor and await its single verdict.
    ///
    /// Opens a fresh `Process` stream, sends exactly one `ProcessingRequest`,
    /// half-closes the send side, and reads the first `ProcessingResponse`.
    ///
    /// The whole exchange is bounded by an explicit `tokio::time::timeout`:
    /// the endpoint's transport `.timeout()` only bounds time-to-response-headers,
    /// so a processor that accepts the stream and then stalls before emitting a
    /// verdict would otherwise hang the request path forever.
    pub async fn exchange(&self, request: ProcessingRequest) -> Result<ProcessingResponse, tonic::Status> {
        let mut client = self.client.clone();
        let fut = async move {
            let outbound = tokio_stream::once(request);
            let mut inbound = client.process(outbound).await?.into_inner();
            match inbound.message().await? {
                Some(resp) => Ok(resp),
                None => Err(tonic::Status::unavailable(
                    "ext_proc processor closed the stream without a response",
                )),
            }
        };
        match tokio::time::timeout(self.timeout, fut).await {
            Ok(result) => result,
            Err(_) => Err(tonic::Status::deadline_exceeded("ext_proc verdict timed out")),
        }
    }
}
