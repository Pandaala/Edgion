//! Canonical resolution, dial-time address classification and TLS for Etcd.
use super::policy::EtcdRuntimePolicy;
use crate::{
    dns::ResolverSpec,
    egress::{EgressPolicy, EgressResolver},
};
use etcd_client::transport::{ConnectionIo, DialFuture, Dialer};
use rustls::pki_types::ServerName;
use std::{collections::HashSet, net::IpAddr, sync::Arc, time::Duration};

pub(crate) struct EtcdDialer {
    resolver: EgressResolver,
    local_resolver: EgressResolver,
    local_seeds: HashSet<String>,
    tls: Option<Arc<rustls::ClientConfig>>,
}
impl EtcdDialer {
    pub fn new(policy: &EtcdRuntimePolicy, egress: EgressPolicy, ttl: Duration) -> Self {
        let local_seeds = policy
            .endpoints
            .iter()
            .filter_map(|uri| {
                let host = uri.host()?.trim_matches(['[', ']']);
                (host.eq_ignore_ascii_case("localhost") || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback()))
                    .then(|| uri.to_string())
            })
            .collect();
        Self {
            resolver: EgressResolver::new(ResolverSpec::System, egress, ttl),
            local_resolver: EgressResolver::new_with_options(ResolverSpec::System, egress, ttl, true, None),
            local_seeds,
            tls: policy.tls.clone(),
        }
    }
}
impl Dialer for EtcdDialer {
    fn dial(&self, uri: http::Uri) -> DialFuture {
        let resolver = if self.local_seeds.contains(&uri.to_string()) {
            &self.local_resolver
        } else {
            &self.resolver
        }
        .clone();
        let tls = self.tls.clone();
        Box::pin(async move {
            let host = uri
                .host()
                .ok_or("Etcd endpoint has no host")?
                .trim_matches(['[', ']'])
                .to_owned();
            let secure = uri.scheme_str() == Some("https");
            let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
            let addrs = resolver
                .resolve_socket_addrs(&host, port)
                .await
                .map_err(|_| "Etcd DNS or egress policy rejected dial")?;
            let mut connected = None;
            for addr in addrs {
                if let Ok(socket) = tokio::net::TcpStream::connect(addr).await {
                    connected = Some(socket);
                    break;
                }
            }
            let socket = connected.ok_or("Etcd TCP connection failed")?;
            socket.set_nodelay(true)?;
            if secure {
                let tls = tls.ok_or("Etcd HTTPS has no TLS configuration")?;
                let name = ServerName::try_from(host).map_err(|_| "invalid Etcd TLS identity")?;
                let stream = tokio_rustls::TlsConnector::from(tls).connect(name, socket).await?;
                if stream.get_ref().1.alpn_protocol() != Some(b"h2") {
                    return Err("Etcd TLS peer did not negotiate h2".into());
                }
                Ok(Box::new(stream) as Box<dyn ConnectionIo>)
            } else {
                Ok(Box::new(socket) as Box<dyn ConnectionIo>)
            }
        })
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/link_sys_core/providers/etcd/dialer_test.rs"]
mod tests;
