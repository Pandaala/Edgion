//! Header Cert Auth plugin implementation.
//!
//! Supports:
//! - `mode=Connection`: read mTLS client cert info from request context
//! - `mode=Header`: parse certificate from request header and verify with configured CA certs

use std::time::Duration;

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use percent_encoding::percent_decode_str;
use pingora_core::tls::hash::MessageDigest;
use pingora_core::tls::x509::{X509Ref, X509};
use x509_parser::parse_x509_certificate;
use x509_parser::pem::Pem;

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::get_secret;
use crate::core::gateway::ctx::ClientCertInfo;
use crate::core::gateway::plugins::http::common::auth_common::send_gateway_auth_error_response;
use crate::core::gateway::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::{CertHeaderFormat, CertSourceMode, ConsumerBy, HeaderCertAuthConfig};

/// Pre-parsed CA entry kept in the trust pool.
///
/// `subject_der` is the DER-encoded `subject` Name, used for O(1) byte
/// comparison against a leaf's `issuer` Name on every request. `cert_der`
/// retains the full CA DER so that on issuer match we can parse it once and
/// hand the SPKI to `verify_signature`.
struct ParsedCa {
    subject_der: Vec<u8>,
    cert_der: Vec<u8>,
}

/// Header Cert Auth request plugin.
pub struct HeaderCertAuth {
    name: String,
    config: HeaderCertAuthConfig,
    ca_pool: Vec<ParsedCa>,
}

impl HeaderCertAuth {
    /// Create plugin instance.
    pub fn new(config: &HeaderCertAuthConfig, plugin_namespace: String) -> Self {
        let ca_pool = Self::load_ca_certs(config, &plugin_namespace);
        Self {
            name: "HeaderCertAuth".to_string(),
            config: config.clone(),
            ca_pool,
        }
    }

    fn load_ca_certs(config: &HeaderCertAuthConfig, plugin_namespace: &str) -> Vec<ParsedCa> {
        let mut raw_ders: Vec<Vec<u8>> = Vec::new();

        if let Some(secrets) = &config.resolved_ca_secrets {
            for secret in secrets {
                if let Some(raw) = Self::read_ca_pem_from_secret(secret) {
                    raw_ders.extend(Self::parse_ca_bundle_to_der(&raw));
                }
            }
        } else {
            for secret_ref in &config.ca_secret_refs {
                let ns = secret_ref.namespace.as_deref().unwrap_or(plugin_namespace);
                let Some(secret) = get_secret(Some(ns), &secret_ref.name) else {
                    continue;
                };
                if let Some(raw) = Self::read_ca_pem_from_secret(&secret) {
                    raw_ders.extend(Self::parse_ca_bundle_to_der(&raw));
                }
            }
        }

        // CA validity is checked only here at init; leaf validity is checked per-request.
        // Drop expired or unparseable CAs and extract subject DER for fast per-request matching.
        let mut pool: Vec<ParsedCa> = Vec::with_capacity(raw_ders.len());
        for cert_der in raw_ders {
            match parse_x509_certificate(&cert_der) {
                Ok((_, cert)) if cert.validity().is_valid() => {
                    let subject_der = cert.subject().as_raw().to_vec();
                    pool.push(ParsedCa { subject_der, cert_der });
                }
                Ok(_) => {
                    tracing::warn!(component = "header_cert_auth", namespace = %plugin_namespace, "Dropping expired CA cert from trust pool");
                }
                Err(_) => {
                    tracing::warn!(component = "header_cert_auth", namespace = %plugin_namespace, "Dropping unparseable CA cert DER from trust pool");
                }
            }
        }
        pool
    }

    fn read_ca_pem_from_secret(secret: &k8s_openapi::api::core::v1::Secret) -> Option<Vec<u8>> {
        if let Some(data) = &secret.data {
            if let Some(value) = data.get("ca.crt") {
                return Some(value.0.clone());
            }
        }
        if let Some(string_data) = &secret.string_data {
            if let Some(value) = string_data.get("ca.crt") {
                return Some(value.as_bytes().to_vec());
            }
        }
        None
    }

    fn parse_ca_bundle_to_der(bundle: &[u8]) -> Vec<Vec<u8>> {
        let mut certs = Vec::new();
        for pem in Pem::iter_from_buffer(bundle).flatten() {
            if pem.label == "CERTIFICATE" && !pem.contents.is_empty() {
                certs.push(pem.contents);
            }
        }

        if certs.is_empty() {
            // Fallback: treat as single DER cert.
            if parse_x509_certificate(bundle).is_ok() {
                certs.push(bundle.to_vec());
            }
        }
        certs
    }

    fn get_identity(&self, cert_info: &ClientCertInfo) -> Option<String> {
        match self.config.consumer_by {
            ConsumerBy::SanOrCn => cert_info
                .sans
                .first()
                .cloned()
                .or_else(|| cert_info.cn.clone())
                .or_else(|| Some(cert_info.subject.clone())),
            ConsumerBy::Cn => cert_info.cn.clone().or_else(|| Some(cert_info.subject.clone())),
            ConsumerBy::Fingerprint => Some(cert_info.fingerprint.clone()),
        }
    }

    fn contains_header_control_chars(value: &str) -> bool {
        value.bytes().any(|b| b == b'\r' || b == b'\n' || b == b'\0')
    }

    fn set_anonymous_headers(&self, session: &mut dyn PluginSession) {
        let _ = session.set_request_header("X-Anonymous-Consumer", "true");
        let _ = session.set_request_header(&self.config.upstream_headers.consumer_header, "anonymous");
    }

    fn apply_headers(&self, session: &mut dyn PluginSession, cert_info: &ClientCertInfo) -> Result<(), &'static str> {
        if self.config.skip_consumer_lookup {
            if !Self::contains_header_control_chars(&cert_info.subject) {
                let _ = session.set_request_header(&self.config.upstream_headers.dn_header, &cert_info.subject);
            }
            if let Some(san) = cert_info.sans.first() {
                if !Self::contains_header_control_chars(san) {
                    let _ = session.set_request_header(&self.config.upstream_headers.san_header, san);
                }
            }
        } else {
            let identity = self.get_identity(cert_info).ok_or("identity-not-found")?;
            if Self::contains_header_control_chars(&identity) {
                return Err("invalid-identity");
            }
            let _ = session.set_request_header(&self.config.upstream_headers.consumer_header, &identity);
        }

        if !Self::contains_header_control_chars(&cert_info.fingerprint) {
            let _ =
                session.set_request_header(&self.config.upstream_headers.fingerprint_header, &cert_info.fingerprint);
        }
        Ok(())
    }

    fn extract_from_connection(&self, session: &dyn PluginSession) -> Result<ClientCertInfo, &'static str> {
        session.client_cert_info().ok_or("no-client-cert")
    }

    fn decode_header_cert_to_pem(&self, raw_value: &str) -> Result<Vec<u8>, &'static str> {
        match self.config.certificate_header_format {
            CertHeaderFormat::Base64Encoded => {
                let body: String = raw_value
                    .chars()
                    .filter(|ch| !ch.is_ascii_whitespace())
                    .collect::<String>();
                let der = STANDARD.decode(body).map_err(|_| "invalid-base64-cert")?;
                Ok(Self::der_to_pem(&der).into_bytes())
            }
            CertHeaderFormat::UrlEncoded => {
                let decoded = percent_decode_str(raw_value)
                    .decode_utf8()
                    .map_err(|_| "invalid-url-encoded-cert")?;
                Ok(decoded.into_owned().into_bytes())
            }
        }
    }

    fn der_to_pem(der: &[u8]) -> String {
        let body = STANDARD.encode(der);
        let mut pem = String::with_capacity(body.len() + 64);
        pem.push_str("-----BEGIN CERTIFICATE-----\n");
        for chunk in body.as_bytes().chunks(64) {
            if let Ok(line) = std::str::from_utf8(chunk) {
                pem.push_str(line);
                pem.push('\n');
            }
        }
        pem.push_str("-----END CERTIFICATE-----\n");
        pem
    }

    fn parse_first_cert_der(bytes: &[u8]) -> Result<Vec<u8>, &'static str> {
        for pem in Pem::iter_from_buffer(bytes) {
            match pem {
                Ok(block) if block.label == "CERTIFICATE" && !block.contents.is_empty() => {
                    return Ok(block.contents);
                }
                Ok(_) => continue,
                Err(_) => return Err("invalid-cert-pem"),
            }
        }
        Err("invalid-cert-pem")
    }

    fn verify_against_ca(&self, cert_pem: &[u8]) -> Result<(), &'static str> {
        if self.ca_pool.is_empty() {
            return Err("no-ca-configured");
        }

        let leaf_der = Self::parse_first_cert_der(cert_pem)?;
        let (_, leaf) = parse_x509_certificate(&leaf_der).map_err(|_| "invalid-leaf-cert")?;
        if !leaf.validity().is_valid() {
            return Err("expired-leaf-cert");
        }

        let issuer_raw = leaf.issuer().as_raw();
        for ca in &self.ca_pool {
            if issuer_raw != ca.subject_der.as_slice() {
                continue;
            }
            // CA DER was successfully parsed at init; re-parse here only on issuer match
            // to obtain a borrowed SubjectPublicKeyInfo for signature verification.
            let Ok((_, ca_cert)) = parse_x509_certificate(&ca.cert_der) else {
                continue;
            };
            if leaf.verify_signature(Some(ca_cert.public_key())).is_ok() {
                return Ok(());
            }
        }

        Err("ca-verification-failed")
    }

    fn extract_client_cert_info(cert: &X509Ref) -> ClientCertInfo {
        let mut subject = String::with_capacity(128);
        let mut first = true;
        for entry in cert.subject_name().entries() {
            if !first {
                subject.push_str(", ");
            }
            first = false;
            let key = entry.object().nid().short_name().unwrap_or("?");
            subject.push_str(key);
            subject.push('=');
            match entry.data().as_utf8() {
                Ok(s) => subject.push_str(&s),
                Err(_) => subject.push('?'),
            }
        }

        let cn = cert
            .subject_name()
            .entries()
            .find(|entry| entry.object().nid().short_name().map(|s| s == "CN").unwrap_or(false))
            .and_then(|entry| entry.data().as_utf8().map(|s| s.to_string()).ok());

        let mut sans = Vec::new();
        if let Some(san_ext) = cert.subject_alt_names() {
            for san in san_ext {
                if let Some(dns_name) = san.dnsname() {
                    sans.push(dns_name.to_string());
                }
                if let Some(ip_bytes) = san.ipaddress() {
                    match ip_bytes.len() {
                        4 => sans.push(format!(
                            "IP:{}.{}.{}.{}",
                            ip_bytes[0], ip_bytes[1], ip_bytes[2], ip_bytes[3]
                        )),
                        16 => {
                            let octets: [u8; 16] = match ip_bytes.try_into() {
                                Ok(arr) => arr,
                                Err(_) => continue,
                            };
                            sans.push(format!("IP:{}", std::net::Ipv6Addr::from(octets)));
                        }
                        _ => {}
                    }
                }
                if let Some(email) = san.email() {
                    sans.push(format!("email:{}", email));
                }
                if let Some(uri) = san.uri() {
                    sans.push(format!("uri:{}", uri));
                }
            }
        }

        let fingerprint = cert
            .digest(MessageDigest::sha256())
            .map(|digest| {
                digest
                    .as_ref()
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect::<Vec<_>>()
                    .join(":")
            })
            .unwrap_or_else(|_| "unknown".to_string());

        ClientCertInfo {
            subject,
            sans,
            cn,
            fingerprint,
        }
    }

    fn extract_from_header(&self, session: &dyn PluginSession) -> Result<ClientCertInfo, &'static str> {
        let header_value = session
            .header_value(&self.config.certificate_header_name)
            .ok_or("missing-cert-header")?;
        let cert_pem = self.decode_header_cert_to_pem(header_value.trim())?;
        self.verify_against_ca(&cert_pem)?;
        let cert = X509::from_pem(&cert_pem).map_err(|_| "invalid-cert-pem")?;
        Ok(Self::extract_client_cert_info(&cert))
    }

    async fn apply_auth_failure_delay(&self) {
        if self.config.auth_failure_delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.config.auth_failure_delay_ms)).await;
        }
    }

    async fn reject(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
        detail: &'static str,
    ) -> PluginRunningResult {
        plugin_log.push_err(detail);
        self.apply_auth_failure_delay().await;
        let _ = send_gateway_auth_error_response(
            session,
            self.config.error_status,
            "TLSCert",
            "edgion",
            None,
            &self.config.error_message,
        )
        .await;
        PluginRunningResult::ErrTerminateRequest
    }

    fn push_cert_deny(plugin_log: &mut PluginLog, detail: &'static str) {
        // Decode / load errors (push_err only); known deny labels get a push entry.
        let label = match detail {
            "expired-leaf-cert" => Some("Deny cert expired"),
            "ca-verification-failed" => Some("Deny cert untrusted"),
            "missing-cert-header" | "no-client-cert" => Some("Deny missing cert"),
            // Decode / load failures: push_err in reject() is sufficient.
            _ => None,
        };
        if let Some(l) = label {
            plugin_log.push(l);
        }
    }
}

#[async_trait]
impl RequestFilter for HeaderCertAuth {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        let cert_info = match self.config.mode {
            CertSourceMode::Connection => self.extract_from_connection(session),
            CertSourceMode::Header => self.extract_from_header(session),
        };

        let cert_info = match cert_info {
            Ok(info) => info,
            Err(detail) => {
                if self.config.allow_anonymous {
                    self.set_anonymous_headers(session);
                    if self.config.mode == CertSourceMode::Header && self.config.hide_credentials {
                        let _ = session.remove_request_header(&self.config.certificate_header_name);
                    }
                    plugin_log.push("Allow anonymous");
                    return PluginRunningResult::GoodNext;
                }
                Self::push_cert_deny(plugin_log, detail);
                return self.reject(session, plugin_log, detail).await;
            }
        };

        if let Err(detail) = self.apply_headers(session, &cert_info) {
            if self.config.allow_anonymous {
                self.set_anonymous_headers(session);
                if self.config.mode == CertSourceMode::Header && self.config.hide_credentials {
                    let _ = session.remove_request_header(&self.config.certificate_header_name);
                }
                plugin_log.push("Allow anonymous");
                return PluginRunningResult::GoodNext;
            }
            Self::push_cert_deny(plugin_log, detail);
            return self.reject(session, plugin_log, detail).await;
        }

        if self.config.mode == CertSourceMode::Header && self.config.hide_credentials {
            let _ = session.remove_request_header(&self.config.certificate_header_name);
        }

        plugin_log.push("cert verify:ok");
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::gateway::plugins::runtime::traits::session::MockPluginSession;

    fn sample_cert_info() -> ClientCertInfo {
        ClientCertInfo {
            subject: "CN=alice, O=edgion".to_string(),
            sans: vec!["alice@example.com".to_string()],
            cn: Some("alice".to_string()),
            fingerprint: "aa:bb:cc".to_string(),
        }
    }

    #[tokio::test]
    async fn test_connection_mode_success() {
        let config = HeaderCertAuthConfig {
            mode: CertSourceMode::Connection,
            ..Default::default()
        };
        let plugin = HeaderCertAuth::new(&config, "default".to_string());

        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new("HeaderCertAuth");

        session
            .expect_client_cert_info()
            .times(1)
            .return_const(Some(sample_cert_info()));
        session.expect_set_request_header().times(2).returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(log.contains("cert verify:ok"));
    }

    #[tokio::test]
    async fn test_connection_mode_anonymous_on_missing_cert() {
        let config = HeaderCertAuthConfig {
            mode: CertSourceMode::Connection,
            allow_anonymous: true,
            ..Default::default()
        };
        let plugin = HeaderCertAuth::new(&config, "default".to_string());

        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new("HeaderCertAuth");

        session.expect_client_cert_info().times(1).return_const(None);
        session.expect_set_request_header().times(2).returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(log.contains("Allow anonymous"));
    }

    // Generate a CA cert + a leaf cert signed by that CA, both PEM-encoded.
    fn make_ca_and_leaf(
        ca_not_before_days: i64,
        ca_not_after_days: i64,
        leaf_not_before_days: i64,
        leaf_not_after_days: i64,
    ) -> (String, String) {
        use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};

        let now = ::time::OffsetDateTime::now_utc();

        let ca_key = KeyPair::generate().expect("CA key gen failed");
        let mut ca_params = CertificateParams::new(vec!["Edgion Test CA".to_string()]).expect("CA params failed");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.not_before = now + ::time::Duration::days(ca_not_before_days);
        ca_params.not_after = now + ::time::Duration::days(ca_not_after_days);
        let ca_cert = ca_params.self_signed(&ca_key).expect("CA self-sign failed");
        let ca_issuer = Issuer::from_params(&ca_params, &ca_key);

        let leaf_key = KeyPair::generate().expect("leaf key gen failed");
        let mut leaf_params = CertificateParams::new(vec!["leaf.example.com".to_string()]).expect("leaf params failed");
        leaf_params.not_before = now + ::time::Duration::days(leaf_not_before_days);
        leaf_params.not_after = now + ::time::Duration::days(leaf_not_after_days);
        let leaf_cert = leaf_params.signed_by(&leaf_key, &ca_issuer).expect("leaf sign failed");

        (ca_cert.pem(), leaf_cert.pem())
    }

    fn make_ca_secret(ca_pem: &str) -> k8s_openapi::api::core::v1::Secret {
        let mut data = std::collections::BTreeMap::new();
        data.insert(
            "ca.crt".to_string(),
            k8s_openapi::ByteString(ca_pem.as_bytes().to_vec()),
        );
        k8s_openapi::api::core::v1::Secret {
            data: Some(data),
            ..Default::default()
        }
    }

    fn header_mode_plugin_with_ca(secrets: Vec<k8s_openapi::api::core::v1::Secret>) -> HeaderCertAuth {
        let config = HeaderCertAuthConfig {
            mode: CertSourceMode::Header,
            resolved_ca_secrets: Some(secrets),
            ..Default::default()
        };
        HeaderCertAuth::new(&config, "default".to_string())
    }

    #[test]
    fn test_verify_against_ca_succeeds_with_matching_ca() {
        let (ca_pem, leaf_pem) = make_ca_and_leaf(-1, 30, -1, 30);
        let plugin = header_mode_plugin_with_ca(vec![make_ca_secret(&ca_pem)]);
        assert!(plugin.verify_against_ca(leaf_pem.as_bytes()).is_ok());
    }

    #[test]
    fn test_verify_against_ca_rejects_issuer_mismatch() {
        // Trusted CA in pool, but leaf is signed by a different (untrusted) CA.
        let (trusted_ca_pem, _) = make_ca_and_leaf(-1, 30, -1, 30);
        let (_, foreign_leaf_pem) = make_ca_and_leaf(-1, 30, -1, 30);
        let plugin = header_mode_plugin_with_ca(vec![make_ca_secret(&trusted_ca_pem)]);
        assert_eq!(
            plugin.verify_against_ca(foreign_leaf_pem.as_bytes()).unwrap_err(),
            "ca-verification-failed"
        );
    }

    #[test]
    fn test_verify_against_ca_rejects_expired_leaf() {
        let (ca_pem, expired_leaf_pem) = make_ca_and_leaf(-30, 30, -30, -1);
        let plugin = header_mode_plugin_with_ca(vec![make_ca_secret(&ca_pem)]);
        assert_eq!(
            plugin.verify_against_ca(expired_leaf_pem.as_bytes()).unwrap_err(),
            "expired-leaf-cert"
        );
    }

    #[test]
    fn test_verify_against_ca_no_ca_when_pool_empty() {
        let (_, leaf_pem) = make_ca_and_leaf(-1, 30, -1, 30);
        let config = HeaderCertAuthConfig {
            mode: CertSourceMode::Header,
            ..Default::default()
        };
        let plugin = HeaderCertAuth::new(&config, "default".to_string());
        assert_eq!(
            plugin.verify_against_ca(leaf_pem.as_bytes()).unwrap_err(),
            "no-ca-configured"
        );
    }

    #[test]
    fn test_verify_against_ca_matches_non_first_ca_in_pool() {
        // Pool contains [foreign_ca, trusted_ca]; leaf is signed by trusted_ca.
        // Exercises the loop iterating past a non-matching CA before hitting the match.
        let (foreign_ca_pem, _) = make_ca_and_leaf(-1, 30, -1, 30);
        let (trusted_ca_pem, leaf_pem) = make_ca_and_leaf(-1, 30, -1, 30);
        let plugin = header_mode_plugin_with_ca(vec![make_ca_secret(&foreign_ca_pem), make_ca_secret(&trusted_ca_pem)]);
        assert!(plugin.verify_against_ca(leaf_pem.as_bytes()).is_ok());
    }

    #[test]
    fn test_load_ca_certs_drops_expired_ca_at_init() {
        // Expired CA should be filtered out at init; pool becomes empty.
        let (expired_ca_pem, leaf_pem) = make_ca_and_leaf(-30, -1, -1, 30);
        let plugin = header_mode_plugin_with_ca(vec![make_ca_secret(&expired_ca_pem)]);
        assert_eq!(
            plugin.verify_against_ca(leaf_pem.as_bytes()).unwrap_err(),
            "no-ca-configured"
        );
    }
}
