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
    let url = format!("http://{}:{}{}", addr.ip(), port, path);

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
}
