use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpStream;

use crate::link_sys::http_client::{apply_outbound_policy, OutboundClientConfig, OutboundRedirect};

#[derive(Debug)]
pub enum ProbeError {
    Timeout,
    ConnectionRefused(std::io::Error),
    UnexpectedStatus(u16),
    RequestFailed(reqwest::Error),
    InvalidPath,
    InvalidUri,
    GrpcConnectionFailed(tonic::transport::Error),
    GrpcNotServing(i32),
    GrpcCallFailed(Box<tonic::Status>),
    InvalidTlsConfig,
    Superseded,
    UnsafeEndpoint,
    ClientUnavailable,
}

impl ProbeError {
    /// Stable, bounded failure class suitable for health-check logs.
    pub const fn class(&self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::ConnectionRefused(_) => "connection-refused",
            Self::UnexpectedStatus(_) => "unexpected-status",
            Self::RequestFailed(_) => "request-failed",
            Self::InvalidPath => "invalid-path",
            Self::InvalidUri => "invalid-uri",
            Self::GrpcConnectionFailed(_) => "grpc-connection-failed",
            Self::GrpcNotServing(_) => "grpc-not-serving",
            Self::GrpcCallFailed(_) => "grpc-call-failed",
            Self::InvalidTlsConfig => "invalid-tls-config",
            Self::Superseded => "superseded",
            Self::UnsafeEndpoint => "unsafe-endpoint",
            Self::ClientUnavailable => "client-unavailable",
        }
    }
}

/// gRPC Health Check protocol structures (grpc.health.v1).
/// Hand-written prost definitions — no proto file needed for this standard protocol.
pub(super) mod grpc_health_proto {
    #[derive(prost::Message)]
    pub struct HealthCheckRequest {
        #[prost(string, tag = "1")]
        pub service: String,
    }

    #[derive(prost::Message)]
    pub struct HealthCheckResponse {
        #[prost(int32, tag = "1")]
        pub status: i32,
    }

    /// ServingStatus.SERVING
    pub const SERVING: i32 = 1;
}

/// Build reusable HTTP probe client with per-probe timeout.
pub fn build_http_probe_client(timeout: Duration) -> Result<reqwest::Client, reqwest::Error> {
    apply_outbound_policy(
        reqwest::Client::builder(),
        OutboundClientConfig {
            connect_timeout: Some(timeout),
            total_timeout: Some(timeout),
            disable_proxy: true,
            redirect: OutboundRedirect::Deny,
            ..Default::default()
        },
    )
    .expect("probe client config has no TLS; apply_outbound_policy is infallible here")
    .build()
}

/// HTTP active health probe.
pub async fn probe_http_with_client(
    client: &reqwest::Client,
    addr: SocketAddr,
    path: &str,
    port_override: Option<u16>,
    host: Option<&str>,
    expected_statuses: &[u16],
) -> Result<(), ProbeError> {
    if !path.starts_with('/') {
        return Err(ProbeError::InvalidPath);
    }

    let port = port_override.unwrap_or(addr.port());
    let target = SocketAddr::new(addr.ip(), port);
    let url = format!("http://{}{}", target, path);

    let mut req = client.get(url);
    if let Some(host_header) = host {
        req = req.header("Host", host_header);
    }

    let resp = req.send().await.map_err(|e| {
        if e.is_timeout() {
            ProbeError::Timeout
        } else {
            ProbeError::RequestFailed(e)
        }
    })?;

    let status = resp.status().as_u16();
    if expected_statuses.contains(&status) {
        Ok(())
    } else {
        Err(ProbeError::UnexpectedStatus(status))
    }
}

/// HTTP active health probe.
pub async fn probe_http(
    addr: SocketAddr,
    path: &str,
    port_override: Option<u16>,
    host: Option<&str>,
    expected_statuses: &[u16],
    timeout: Duration,
) -> Result<(), ProbeError> {
    let client = build_http_probe_client(timeout).map_err(ProbeError::RequestFailed)?;
    probe_http_with_client(&client, addr, path, port_override, host, expected_statuses).await
}

/// gRPC Health Check probe (grpc.health.v1.Health/Check).
pub async fn probe_grpc(
    addr: SocketAddr,
    port_override: Option<u16>,
    service_name: &str,
    timeout: Duration,
) -> Result<(), ProbeError> {
    use grpc_health_proto::{HealthCheckRequest, HealthCheckResponse, SERVING};
    use http::uri::Uri;
    use tonic::codec::ProstCodec;

    let port = port_override.unwrap_or(addr.port());
    let target = SocketAddr::new(addr.ip(), port);
    let uri: Uri = format!("http://{}", target)
        .parse()
        .map_err(|_| ProbeError::InvalidUri)?;

    let channel = tokio::time::timeout(
        timeout,
        tonic::transport::Endpoint::from(uri)
            .timeout(timeout)
            .connect_timeout(timeout)
            .connect(),
    )
    .await
    .map_err(|_| ProbeError::Timeout)?
    .map_err(ProbeError::GrpcConnectionFailed)?;

    let mut grpc_client = tonic::client::Grpc::new(channel);
    grpc_client.ready().await.map_err(ProbeError::GrpcConnectionFailed)?;

    let request = tonic::Request::new(HealthCheckRequest {
        service: service_name.to_string(),
    });

    let path = http::uri::PathAndQuery::from_static("/grpc.health.v1.Health/Check");
    let codec: ProstCodec<HealthCheckRequest, HealthCheckResponse> = ProstCodec::default();

    let response = tokio::time::timeout(timeout, grpc_client.unary(request, path, codec))
        .await
        .map_err(|_| ProbeError::Timeout)?
        .map_err(|error| ProbeError::GrpcCallFailed(Box::new(error)))?;

    let status = response.into_inner().status;
    if status == SERVING {
        Ok(())
    } else {
        Err(ProbeError::GrpcNotServing(status))
    }
}

/// TCP active health probe.
pub async fn probe_tcp(addr: SocketAddr, port_override: Option<u16>, timeout: Duration) -> Result<(), ProbeError> {
    let port = port_override.unwrap_or(addr.port());
    let target = SocketAddr::new(addr.ip(), port);

    tokio::time::timeout(timeout, TcpStream::connect(target))
        .await
        .map_err(|_| ProbeError::Timeout)?
        .map_err(ProbeError::ConnectionRefused)?;

    Ok(())
}

#[cfg(test)]
#[path = "../../../../tests/unit/backends/health/check/probes_test.rs"]
mod tests;
