//! Bounded all-or-nothing member client-URL discovery.
use super::policy::EtcdRuntimePolicy;
use anyhow::Result;
use edgion_resources::resources::link_sys::etcd::{normalize_etcd_endpoint, MAX_ETCD_ENDPOINTS};
use etcd_client::transport::AdmittedEndpointSet;

pub(crate) async fn refresh(set: &AdmittedEndpointSet, policy: &EtcdRuntimePolicy) -> Result<()> {
    let mut client = set.client()?;
    let response = tokio::time::timeout(policy.probe_timeout, client.member_list()).await??;
    let endpoints = normalize_client_urls(
        response
            .members()
            .iter()
            .flat_map(|member| member.client_urls().iter().map(String::as_str)),
        policy.require_tls,
        &set.endpoints(),
    )?;
    set.replace_endpoints(endpoints).await?;
    Ok(())
}

fn normalize_client_urls<'a>(
    urls: impl IntoIterator<Item = &'a str>,
    bootstrap_requires_tls: bool,
    current_endpoints: &[http::Uri],
) -> Result<Vec<http::Uri>> {
    let require_tls = bootstrap_requires_tls || current_endpoints.iter().any(|uri| uri.scheme_str() == Some("https"));
    let mut endpoints = Vec::new();
    for url in urls {
        let uri: http::Uri = normalize_etcd_endpoint(url).map_err(anyhow::Error::msg)?.parse()?;
        anyhow::ensure!(
            !require_tls || uri.scheme_str() == Some("https"),
            "Etcd discovery rejected a TLS downgrade"
        );
        if !endpoints.contains(&uri) {
            endpoints.push(uri);
        }
        anyhow::ensure!(
            endpoints.len() <= MAX_ETCD_ENDPOINTS,
            "Etcd discovery endpoint count exceeded"
        );
    }
    anyhow::ensure!(!endpoints.is_empty(), "Etcd discovery returned no usable client URLs");
    anyhow::ensure!(
        endpoints
            .iter()
            .all(|uri| uri.scheme_str() == endpoints[0].scheme_str()),
        "Etcd discovery rejected mixed HTTP and HTTPS client URLs"
    );
    Ok(endpoints)
}
#[cfg(test)]
#[path = "../../../../tests/unit/link_sys_core/providers/etcd/discovery_test.rs"]
mod tests;
