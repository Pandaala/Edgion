//! Request-scoped probe identity and TLS configuration. No clients survive a fan-out.

use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use edgion_common::link_sys_core::outbound_tls::resolve_outbound_tls_with_defaults;
use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
use edgion_resources::resources::common::{OutboundTlsConfig, OutboundTlsLocal};
use k8s_openapi::api::core::v1::Secret;

use crate::backends::discovery::{get_service_discovery_coordinator, ServiceSelectorGeneration};
use crate::backends::policy::{get_global_backend_tls_policy_store, BackendTlsPolicyLookup};
use crate::backends::{get_global_gateway_backend_tls_store, validate_endpoint_backend_addr};
use crate::link_sys::http_client::{build_outbound_client, OutboundClientConfig, OutboundRedirect};
use crate::plugins::http::common::outbound::pinned_resolver::EndpointPinnedResolver;
use crate::tls::validation::build_verified_outbound_tls;

pub(super) struct ProbeTransport {
    lookup: BackendTlsPolicyLookup,
    discovery: Arc<ServiceSelectorGeneration>,
    policy_generation: u64,
    gateway_key: Option<String>,
    gateway_identity: Option<Arc<Secret>>,
    tls: OnceLock<Result<Option<rustls::ClientConfig>, &'static str>>,
}

impl ProbeTransport {
    pub(super) fn new(
        lookup: BackendTlsPolicyLookup,
        discovery: Arc<ServiceSelectorGeneration>,
        policy_generation: u64,
        gateway_key: Option<&str>,
    ) -> Self {
        let needs_gateway_identity = lookup
            .effective()
            .is_some_and(|effective| effective.policy.spec.resolved_client_certificate.is_none());
        let gateway_key = gateway_key.filter(|_| needs_gateway_identity).map(str::to_owned);
        let gateway_identity = gateway_key
            .as_deref()
            .and_then(|key| get_global_gateway_backend_tls_store().get(key));
        Self {
            lookup,
            discovery,
            policy_generation,
            gateway_key,
            gateway_identity,
            tls: OnceLock::new(),
        }
    }

    fn is_current(&self) -> bool {
        if !Arc::ptr_eq(&self.discovery, &get_service_discovery_coordinator().load_generation())
            || self.policy_generation != get_global_backend_tls_policy_store().snapshot().generation()
        {
            return false;
        }
        if let Some(key) = &self.gateway_key {
            let current = get_global_gateway_backend_tls_store().get(key);
            match (&self.gateway_identity, current) {
                (Some(captured), Some(current)) => Arc::ptr_eq(captured, &current),
                (None, None) => true,
                _ => false,
            }
        } else {
            true
        }
    }

    fn build_tls(&self) -> Result<Option<rustls::ClientConfig>, &'static str> {
        let effective = match &self.lookup {
            BackendTlsPolicyLookup::Absent => return Ok(None),
            BackendTlsPolicyLookup::Invalid => return Err("probe configuration unavailable"),
            BackendTlsPolicyLookup::Effective(effective) => effective,
        };
        let spec = &effective.policy.spec;
        let local = OutboundTlsLocal {
            verify: Some(true),
            validation: Some(OutboundTlsValidation {
                ca_certificate_refs: spec.validation.ca_certificate_refs.clone(),
                hostname: Some(spec.validation.hostname.clone()),
                subject_alt_names: spec.validation.subject_alt_names.clone(),
                well_known_ca_certificates: spec.validation.well_known_ca_certificates.clone(),
            }),
            ..Default::default()
        };
        // Backend policy / Gateway identity owns this exchange. LinkSys global
        // defaults must never inject another trust root or client credential.
        let resolved = resolve_outbound_tls_with_defaults(
            &local,
            spec.resolved_ca_certificates.as_deref(),
            spec.resolved_client_certificate
                .as_ref()
                .or(self.gateway_identity.as_deref()),
            false,
            &OutboundTlsConfig::default(),
        )
        .map_err(|_| "probe configuration unavailable")?;
        build_verified_outbound_tls(
            &resolved,
            spec.validation.subject_alt_names.clone().unwrap_or_default(),
            "AllEndpointStatus",
        )
        .map(Some)
        .map_err(|_| "probe configuration unavailable")
    }

    pub(super) fn client_and_url(
        &self,
        address: SocketAddr,
        path: &str,
        query: Option<&str>,
        allow_loopback: bool,
    ) -> Result<(reqwest::Client, reqwest::Url), &'static str> {
        if !self.is_current() {
            return Err("probe configuration changed");
        }
        validate_endpoint_backend_addr(
            &pingora_core::protocols::l4::socket::SocketAddr::Inet(address),
            allow_loopback,
        )
        .map_err(|_| "unsafe upstream")?;
        let tls = self
            .tls
            .get_or_init(|| self.build_tls())
            .as_ref()
            .map_err(|error| *error)?;
        let hostname = match &self.lookup {
            BackendTlsPolicyLookup::Absent => None,
            BackendTlsPolicyLookup::Effective(effective) => Some(effective.policy.spec.validation.hostname.as_str()),
            BackendTlsPolicyLookup::Invalid => return Err("probe configuration unavailable"),
        };
        let url = probe_url(address, hostname, path, query)?;
        let client = build_outbound_client(OutboundClientConfig {
            pool_max_idle_per_host: Some(0),
            connect_timeout: Some(Duration::from_secs(3)),
            disable_proxy: true,
            redirect: OutboundRedirect::Deny,
            preconfigured_tls: tls.clone(),
            dns_resolver: Some(Arc::new(EndpointPinnedResolver::new(address, allow_loopback))),
            ..Default::default()
        })
        .map_err(|_| "probe client unavailable")?;
        Ok((client, url))
    }
}

fn probe_url(
    address: SocketAddr,
    hostname: Option<&str>,
    path: &str,
    query: Option<&str>,
) -> Result<reqwest::Url, &'static str> {
    let mut url = reqwest::Url::parse(if hostname.is_some() {
        "https://placeholder.invalid"
    } else {
        "http://placeholder.invalid"
    })
    .map_err(|_| "invalid probe request")?;
    if let Some(hostname) = hostname {
        url.set_host(Some(hostname))
            .map_err(|_| "probe configuration unavailable")?;
        // reqwest bypasses its resolver for IP literals, including numeric
        // spellings normalized by URL parsing. Such a host would escape the pin.
        if !matches!(url.host(), Some(url::Host::Domain(_))) {
            return Err("probe configuration unavailable");
        }
    } else {
        url.set_ip_host(address.ip()).map_err(|_| "invalid probe request")?;
    }
    url.set_port(Some(address.port()))
        .map_err(|_| "invalid probe request")?;
    // Setters keep path/query input from changing authority or scheme.
    url.set_path(path);
    url.set_query(query);
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::backend_tls_policy::{
        BackendTLSPolicyCACertificateRef, BackendTLSPolicySpec, BackendTLSPolicyValidation,
    };
    use edgion_resources::resources::BackendTLSPolicy;
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    #[test]
    fn tls_hostname_cannot_bypass_the_endpoint_pin_as_a_numeric_host() {
        let address = "10.0.0.1:8443".parse().unwrap();
        for hostname in ["127.0.0.1", "2130706433", "0x7f000001", "0177.0.0.1", "[::1]"] {
            assert_eq!(
                probe_url(address, Some(hostname), "/health", None).unwrap_err(),
                "probe configuration unavailable"
            );
        }
        let url = probe_url(
            address,
            Some("backend.example.com"),
            "//other.example/path",
            Some("a=b#fragment"),
        )
        .unwrap();
        assert_eq!(url.host_str(), Some("backend.example.com"));
        assert_eq!(url.port(), Some(8443));
        assert_eq!(url.path(), "//other.example/path");
        assert_eq!(url.query(), Some("a=b%23fragment"));
        assert!(url.fragment().is_none());
    }

    #[test]
    fn plaintext_ipv6_uses_the_proven_socket_and_preserves_escaped_paths() {
        let url = probe_url("[2001:db8::1]:8080".parse().unwrap(), None, "/foo%2Fbar", None).unwrap();
        assert_eq!(url.as_str(), "http://[2001:db8::1]:8080/foo%2Fbar");
    }

    #[test]
    fn mixed_valid_and_empty_ca_maps_to_generic_probe_configuration_error() {
        let certificate = rcgen::generate_simple_self_signed(vec!["backend.example.com".to_string()])
            .expect("generate probe CA certificate");
        let ca_secret = |pem: &[u8]| Secret {
            data: Some(BTreeMap::from([("ca.crt".to_string(), ByteString(pem.to_vec()))])),
            ..Default::default()
        };
        let policy = BackendTLSPolicy {
            metadata: Default::default(),
            spec: BackendTLSPolicySpec {
                current_status: None,
                target_refs: Vec::new(),
                resolved_target_class: None,
                validation: BackendTLSPolicyValidation {
                    ca_certificate_refs: Some(
                        ["ca-valid", "ca-empty"]
                            .map(|name| BackendTLSPolicyCACertificateRef {
                                group: String::new(),
                                kind: "Secret".to_string(),
                                name: name.to_string(),
                                namespace: None,
                            })
                            .to_vec(),
                    ),
                    hostname: "backend.example.com".to_string(),
                    subject_alt_names: None,
                    well_known_ca_certificates: None,
                },
                options: None,
                resolved_ca_certificates: Some(vec![ca_secret(certificate.cert.pem().as_bytes()), ca_secret(b"")]),
                resolved_client_certificate: None,
                use_system_ca: false,
            },
            status: None,
        };
        let transport = ProbeTransport::new(
            BackendTlsPolicyLookup::effective_from_policy(Arc::new(policy)),
            get_service_discovery_coordinator().load_generation(),
            get_global_backend_tls_policy_store().snapshot().generation(),
            None,
        );

        assert_eq!(transport.build_tls().unwrap_err(), "probe configuration unavailable");
    }
}
