use std::net::SocketAddr;

use pingora_core::protocols::l4::proxy_protocol::ProxyProtocolTrust;

use super::*;

#[derive(Debug)]
struct AlwaysTrust;

impl ProxyProtocolTrust for AlwaysTrust {
    fn is_trusted(&self, _addr: &SocketAddr) -> bool {
        true
    }
}

#[test]
fn socket_mapping_has_one_mandatory_none_case() {
    assert!(tcp_sock_opts_for_policy(&PreparedProxyProtocolPolicy::Disabled).is_none());

    let required = tcp_sock_opts_for_policy(&PreparedProxyProtocolPolicy::Required).unwrap();
    assert!(required.proxy_protocol);
    assert!(required.proxy_protocol_trusted_sources.is_none());

    let trusted = tcp_sock_opts_for_policy(&PreparedProxyProtocolPolicy::TrustedSources {
        trust: Arc::new(AlwaysTrust),
        cidr_count: 1,
    })
    .unwrap();
    assert!(trusted.proxy_protocol);
    assert!(trusted.proxy_protocol_trusted_sources.is_some());
}
