use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpStream;

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
    GrpcCallFailed(tonic::Status),
}

/// gRPC Health Check protocol structures (grpc.health.v1).
/// Hand-written prost definitions — no proto file needed for this standard protocol.
mod grpc_health_proto {
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
    reqwest::Client::builder()
        .connect_timeout(timeout)
        .timeout(timeout)
        .no_proxy()
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
        .map_err(ProbeError::GrpcCallFailed)?;

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
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn test_tcp_probe_success() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        tokio::spawn(async move {
            let _ = listener.accept().await;
        });

        let res = probe_tcp(addr, None, Duration::from_secs(1)).await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_tcp_probe_failure_connection_refused() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        drop(listener);

        let res = probe_tcp(addr, None, Duration::from_millis(300)).await;
        assert!(matches!(
            res,
            Err(ProbeError::ConnectionRefused(_)) | Err(ProbeError::Timeout)
        ));
    }

    #[tokio::test]
    async fn test_http_probe_matching_status() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut req_buf = [0_u8; 1024];
                let _ = stream.read(&mut req_buf).await;
                let _ = stream
                    .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await;
            }
        });

        let res = probe_http(addr, "/healthz", None, None, &[200, 204], Duration::from_secs(1)).await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_http_probe_unexpected_status() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut req_buf = [0_u8; 1024];
                let _ = stream.read(&mut req_buf).await;
                let _ = stream
                    .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await;
            }
        });

        let res = probe_http(addr, "/healthz", None, None, &[200], Duration::from_secs(1)).await;
        assert!(matches!(res, Err(ProbeError::UnexpectedStatus(503))));
    }

    #[tokio::test]
    async fn test_http_probe_ipv6_url_is_bracketed() {
        // Bind an IPv6 loopback listener; skip if the host has no IPv6 support.
        let Ok(listener) = TcpListener::bind("[::1]:0").await else {
            eprintln!("skipping test_http_probe_ipv6_url_is_bracketed: IPv6 loopback unavailable");
            return;
        };
        let addr = listener.local_addr().expect("listener addr");
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut req_buf = [0_u8; 1024];
                let _ = stream.read(&mut req_buf).await;
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await;
            }
        });

        let res = probe_http(addr, "/healthz", None, None, &[200], Duration::from_secs(1)).await;
        assert!(res.is_ok(), "IPv6 probe should succeed, got {res:?}");
    }

    #[tokio::test]
    async fn test_grpc_probe_ipv6_does_not_panic() {
        // No gRPC server is listening; the probe must return an error rather than panic.
        // No working IPv6 stack required: the URI must parse (bracketed `::1`) and the
        // failure must come from the network layer, not URI construction.
        let addr: SocketAddr = "[::1]:1".parse().expect("parse ipv6 addr");
        let res = probe_grpc(addr, None, "", Duration::from_millis(300)).await;
        assert!(matches!(
            res,
            Err(ProbeError::GrpcConnectionFailed(_)) | Err(ProbeError::Timeout)
        ));
    }
}
