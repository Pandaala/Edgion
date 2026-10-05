//! Immutable, validated Etcd policy prepared once per provider generation.
use super::config_mapping::{admitted_connect_options, health_probe_timeout};
use crate::link_sys_core::{credential::CredentialState, tls::build_required_rustls_client_config};
use anyhow::{Context, Result};
use edgion_resources::{
    credentials::materials::BasicAuthMaterial,
    resources::{
        common::TlsConfig,
        link_sys::etcd::{normalize_etcd_endpoint, EtcdClientConfig},
    },
};
use etcd_client::{transport::AdmissionPolicy, ConnectOptions};
use std::{sync::Arc, time::Duration};

pub(crate) struct EtcdRuntimePolicy {
    pub endpoints: Vec<http::Uri>,
    pub options: ConnectOptions,
    pub admission: AdmissionPolicy,
    pub sync_interval: Option<Duration>,
    pub probe_timeout: Duration,
    pub tls: Option<Arc<rustls::ClientConfig>>,
    pub require_tls: bool,
}
impl EtcdRuntimePolicy {
    pub fn resolve(config: &EtcdClientConfig, auth: &CredentialState<BasicAuthMaterial>) -> Result<Self> {
        let errors = config.validation_errors();
        anyhow::ensure!(errors.is_empty(), "Invalid Etcd configuration: {}", errors.join("; "));
        let mut endpoints = Vec::new();
        for endpoint in &config.endpoints {
            let endpoint: http::Uri = normalize_etcd_endpoint(endpoint).map_err(anyhow::Error::msg)?.parse()?;
            if !endpoints.contains(&endpoint) {
                endpoints.push(endpoint);
            }
        }
        let local_tls = config.tls.as_ref().filter(|tls| tls.enabled);
        if local_tls.is_some() {
            anyhow::ensure!(
                endpoints.iter().all(|uri| uri.scheme_str() == Some("https")),
                "Etcd TLS configuration requires HTTPS endpoints"
            );
        }
        let require_tls = endpoints.iter().any(|uri| uri.scheme_str() == Some("https"));
        let tls = if require_tls || config.auto_sync_interval.is_some_and(|interval| interval > 0) {
            let default = TlsConfig::default();
            let mut tls = build_required_rustls_client_config(local_tls.unwrap_or(&default))?;
            tls.alpn_protocols = vec![b"h2".to_vec()];
            Some(Arc::new(tls))
        } else {
            None
        };
        let probe_timeout = health_probe_timeout(config);
        let options = admitted_connect_options(config, auth)?;
        let admission = AdmissionPolicy {
            timeout: probe_timeout.max(Duration::from_millis(
                config
                    .timeout
                    .as_ref()
                    .and_then(|timeout| timeout.dial)
                    .unwrap_or(5000)
                    .min(super::config_mapping::MAX_DIAL_TIMEOUT_MS),
            )),
            minimum_version: config
                .parsed_min_cluster_version()
                .map_err(anyhow::Error::msg)
                .context("invalid Etcd version policy")?,
            cluster_id: None,
        };
        Ok(Self {
            endpoints,
            options,
            admission,
            sync_interval: config
                .auto_sync_interval
                .filter(|interval| *interval > 0)
                .map(Duration::from_secs),
            probe_timeout,
            tls,
            require_tls,
        })
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/link_sys_core/providers/etcd/policy_test.rs"]
mod tests;
