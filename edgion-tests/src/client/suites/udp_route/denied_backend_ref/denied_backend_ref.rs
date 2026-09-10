// UDPRoute DeniedBackendRef suite: a cross-namespace Service backendRef
// without a ReferenceGrant must drop the datagram (no echo).
//
// Local fixtures use edgion-test -> backend; Kubernetes fixtures use:
// - 31955 (udp): UDPRoute in edgion-default -> Service edgion-backend/udp-denied-ref
//
// The backend EndpointSlice is real (test_server :30011). If the post-select
// gate is missing, the client receives an echo and the case fails.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::status::helpers;
use async_trait::async_trait;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;

pub struct UdpDeniedBackendRefTestSuite;

fn unspecified_bind_addr_for(target: SocketAddr) -> SocketAddr {
    let ip = match target.ip() {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
    };
    SocketAddr::new(ip, 0)
}

async fn resolve_udp_target(ctx: &TestContext) -> Result<SocketAddr, String> {
    tokio::net::lookup_host(ctx.udp_addr())
        .await
        .map_err(|error| format!("UDP target resolution failed: {error}"))?
        .next()
        .ok_or_else(|| "UDP target resolution returned no addresses".to_string())
}

async fn assert_attached_and_denied(ctx: &TestContext) -> Result<(), String> {
    helpers::wait_resource_status(
        ctx,
        "UDPRoute",
        helpers::l4_denied_backend_ref_namespace(),
        "udp-denied-ref",
        Duration::from_secs(10),
        |route| {
            route["status"]["parents"].as_array().is_some_and(|parents| {
                parents.iter().any(|parent| {
                    let conditions = parent["conditions"].as_array();
                    conditions.is_some_and(|conditions| {
                        conditions
                            .iter()
                            .any(|condition| condition["type"] == "Accepted" && condition["status"] == "True")
                            && conditions.iter().any(|condition| {
                                condition["type"] == "ResolvedRefs"
                                    && condition["status"] == "False"
                                    && condition["reason"] == "RefNotPermitted"
                            })
                    })
                })
            })
        },
    )
    .await
    .map(|_| ())
}

impl UdpDeniedBackendRefTestSuite {
    fn test_denied_ref_does_not_echo() -> TestCase {
        TestCase::new(
            "denied_cross_ns_ref_fail_closed",
            "UDPRoute cross-ns backendRef without ReferenceGrant must drop the packet",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if let Err(error) = assert_attached_and_denied(&ctx).await {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("route attachment precondition failed: {error}"),
                        );
                    }
                    let payload = b"must-not-echo";

                    let target = match resolve_udp_target(&ctx).await {
                        Ok(target) => target,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };

                    let socket = match UdpSocket::bind(unspecified_bind_addr_for(target)).await {
                        Ok(s) => s,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("UDP bind failed: {e}")),
                    };
                    if let Err(e) = socket.send_to(payload, target).await {
                        return TestResult::failed(start.elapsed(), format!("UDP send failed: {e}"));
                    }

                    let mut buf = vec![0u8; 64];
                    match tokio::time::timeout(Duration::from_secs(2), socket.recv_from(&mut buf)).await {
                        Err(_) => TestResult::passed_with_message(
                            start.elapsed(),
                            "Denied backendRef dropped the datagram".to_string(),
                        ),
                        Ok(Ok((n, _))) => TestResult::failed(
                            start.elapsed(),
                            format!("Expected drop but received {n} bytes (gate missing?)"),
                        ),
                        Ok(Err(e)) => TestResult::failed(start.elapsed(), format!("UDP recv failed: {e}")),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for UdpDeniedBackendRefTestSuite {
    fn name(&self) -> &str {
        "UDPRoute DeniedBackendRef"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_denied_ref_does_not_echo()]
    }
}

#[cfg(test)]
mod tests {
    use super::unspecified_bind_addr_for;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

    #[test]
    fn udp_bind_address_matches_resolved_target_family() {
        assert_eq!(
            unspecified_bind_addr_for(SocketAddr::from((Ipv4Addr::new(10, 96, 0, 10), 31955))),
            SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))
        );
        assert_eq!(
            unspecified_bind_addr_for(SocketAddr::from((Ipv6Addr::LOCALHOST, 31955))),
            SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0))
        );
    }
}
