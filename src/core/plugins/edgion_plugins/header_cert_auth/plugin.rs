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

use crate::core::conf_mgr::sync_runtime::resource_processor::get_secret;
use crate::core::plugins::edgion_plugins::common::auth_common::send_auth_error_response;
use crate::core::plugins::plugin_runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::ctx::ClientCertInfo;
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::{CertHeaderFormat, CertSourceMode, ConsumerBy, HeaderCertAuthConfig};

/// Header Cert Auth request plugin.
pub struct HeaderCertAuth {
    name: String,
    config: HeaderCertAuthConfig,
    plugin_namespace: String,
    ca_certs_der: Vec<Vec<u8>>,
}

impl HeaderCertAuth {
    /// Create plugin instance.
    pub fn new(config: &HeaderCertAuthConfig, plugin_namespace: String) -> Self {
        let ca_certs_der = Self::load_ca_certs(config, &plugin_namespace);
        Self {
            name: "HeaderCertAuth".to_string(),
            config: config.clone(),
            plugin_namespace,
            ca_certs_der,
        }
    }

    fn load_ca_certs(config: &HeaderCertAuthConfig, plugin_namespace: &str) -> Vec<Vec<u8>> {
        let mut out = Vec::new();

        if let Some(secrets) = &config.resolved_ca_secrets {
            for secret in secrets {
                if let Some(raw) = Self::read_ca_pem_from_secret(secret) {
                    out.extend(Self::parse_ca_bundle_to_der(&raw));
                }
            }
            return out;
        }

        for secret_ref in &config.ca_secret_refs {
            let ns = secret_ref.namespace.as_deref().unwrap_or(plugin_namespace);
            let Some(secret) = get_secret(Some(ns), &secret_ref.name) else {
                continue;
            };
            if let Some(raw) = Self::read_ca_pem_from_secret(&secret) {
                out.extend(Self::parse_ca_bundle_to_der(&raw));
            }
        }

        out
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
        if self.ca_certs_der.is_empty() {
            return Err("no-ca-configured");
        }

        let leaf_der = Self::parse_first_cert_der(cert_pem)?;
        let (_, leaf) = parse_x509_certificate(&leaf_der).map_err(|_| "invalid-leaf-cert")?;
        if !leaf.validity().is_valid() {
            return Err("expired-leaf-cert");
        }

        for ca_der in &self.ca_certs_der {
            let Ok((_, ca)) = parse_x509_certificate(ca_der) else {
                continue;
            };
            if !ca.validity().is_valid() {
                continue;
            }
            if leaf.issuer() != ca.subject() {
                continue;
            }
            if leaf.verify_signature(Some(ca.public_key())).is_ok() {
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
        detail: &str,
    ) -> PluginRunningResult {
        tracing::debug!(
            detail = detail,
            mode = ?self.config.mode,
            namespace = %self.plugin_namespace,
            "HeaderCertAuth authentication failed"
        );
        plugin_log.push("hca:fail");
        self.apply_auth_failure_delay().await;
        let _ = send_auth_error_response(
            session,
            self.config.error_status,
            "TLSCert",
            "edgion",
            &self.config.error_message,
        )
        .await;
        PluginRunningResult::ErrTerminateRequest
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
                    plugin_log.push("hca:anonymous");
                    return PluginRunningResult::GoodNext;
                }
                return self.reject(session, plugin_log, detail).await;
            }
        };

        if let Err(detail) = self.apply_headers(session, &cert_info) {
            if self.config.allow_anonymous {
                self.set_anonymous_headers(session);
                plugin_log.push("hca:anonymous");
                return PluginRunningResult::GoodNext;
            }
            return self.reject(session, plugin_log, detail).await;
        }

        if self.config.mode == CertSourceMode::Header && self.config.hide_credentials {
            let _ = session.remove_request_header(&self.config.certificate_header_name);
        }

        plugin_log.push("hca:ok");
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::plugins::plugin_runtime::traits::session::MockPluginSession;

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
        assert!(log.contains("hca:ok"));
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
        assert!(log.contains("hca:anonymous"));
    }
}
