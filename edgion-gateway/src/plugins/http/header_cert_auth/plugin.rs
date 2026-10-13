//! Header Cert Auth plugin implementation.
//!
//! Supports:
//! - `mode=Connection`: read mTLS client cert info from request context
//! - `mode=Header`: parse certificate from request header and verify with configured CA certs

use crate::plugins::http::common::auth_common::set_sensitive_request_header;
use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use percent_encoding::percent_decode_str;
use pingora_core::tls::x509::X509;
use x509_parser::parse_x509_certificate;
use x509_parser::pem::Pem;

use crate::ctx::ClientCertInfo;
use crate::plugins::http::common::auth_common::{apply_auth_failure_delay, send_gateway_auth_error_response};
use crate::plugins::runtime::{PluginLog, PluginSession, PluginSessionHeaderExt, RequestFilter};
use crate::tls::runtime::backend::cert_extractor::extract_client_cert_info_from_cert;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{CertHeaderFormat, CertSourceMode, ConsumerBy, HeaderCertAuthConfig};
use edgion_resources::resources::refs::plugin_fields::BoundHeaderCertAuth;

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
    pub fn new(field: &BoundHeaderCertAuth<'_>) -> Self {
        let ca_pool = Self::load_ca_certs(field);
        let config = field.config();
        let mut config = config.clone();
        config.resolved_ca_secrets = None;
        Self {
            name: "HeaderCertAuth".to_string(),
            config,
            ca_pool,
        }
    }

    fn load_ca_certs(field: &BoundHeaderCertAuth<'_>) -> Vec<ParsedCa> {
        let plugin_namespace = field.owner_namespace();
        let mut raw_ders: Vec<Vec<u8>> = Vec::new();
        for pair in field.ca_materials() {
            if let Some(raw) = pair.material().and_then(Self::read_ca_pem_from_secret) {
                raw_ders.extend(Self::parse_ca_bundle_to_der(raw));
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

    fn read_ca_pem_from_secret(secret: &k8s_openapi::api::core::v1::Secret) -> Option<&[u8]> {
        if let Some(data) = &secret.data {
            if let Some(value) = data.get("ca.crt") {
                return Some(&value.0);
            }
        }
        if let Some(string_data) = &secret.string_data {
            if let Some(value) = string_data.get("ca.crt") {
                return Some(value.as_bytes());
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

    fn get_identity<'a>(&self, cert_info: &'a ClientCertInfo) -> Option<&'a str> {
        match self.config.consumer_by {
            ConsumerBy::SanOrCn => cert_info.sans.first().map(String::as_str).or(cert_info.cn.as_deref()),
            ConsumerBy::Cn => cert_info.cn.as_deref(),
            ConsumerBy::Fingerprint => Some(cert_info.fingerprint.as_str()),
        }
    }

    fn contains_header_control_chars(value: &str) -> bool {
        value.bytes().any(|b| b == b'\r' || b == b'\n' || b == b'\0')
    }

    fn set_anonymous_headers(&self, session: &mut dyn PluginSession) {
        let _ = set_sensitive_request_header(session, "X-Anonymous-Consumer", "true");
        let _ = set_sensitive_request_header(session, &self.config.upstream_headers.consumer_header, "anonymous");
    }

    fn apply_headers(&self, session: &mut dyn PluginSession, cert_info: &ClientCertInfo) -> Result<(), &'static str> {
        if self.config.skip_consumer_lookup {
            if !Self::contains_header_control_chars(&cert_info.subject) {
                let _ =
                    set_sensitive_request_header(session, &self.config.upstream_headers.dn_header, &cert_info.subject);
            }
            if let Some(san) = cert_info.sans.first() {
                if !Self::contains_header_control_chars(san) {
                    let _ = set_sensitive_request_header(session, &self.config.upstream_headers.san_header, san);
                }
            }
        } else {
            let identity = self.get_identity(cert_info).ok_or("identity-not-found")?;
            if Self::contains_header_control_chars(identity) {
                return Err("invalid-identity");
            }
            let _ = set_sensitive_request_header(session, &self.config.upstream_headers.consumer_header, identity);
        }

        if !Self::contains_header_control_chars(&cert_info.fingerprint) {
            let _ = set_sensitive_request_header(
                session,
                &self.config.upstream_headers.fingerprint_header,
                &cert_info.fingerprint,
            );
        }
        Ok(())
    }

    fn extract_from_connection(&self, session: &dyn PluginSession) -> Result<ClientCertInfo, &'static str> {
        session.client_cert_info().cloned().ok_or("no-client-cert")
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
        self.verify_leaf_der(&leaf_der)
    }

    fn verify_leaf_der(&self, leaf_der: &[u8]) -> Result<(), &'static str> {
        if self.ca_pool.is_empty() {
            return Err("no-ca-configured");
        }
        if leaf_der.is_empty() {
            return Err("invalid-cert-pem");
        }

        let (_, leaf) = parse_x509_certificate(leaf_der).map_err(|_| "invalid-leaf-cert")?;
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

    fn extract_from_header(&self, session: &dyn PluginSession) -> Result<ClientCertInfo, &'static str> {
        let header_value = session
            .header_str(&self.config.certificate_header_name)
            .ok_or("missing-cert-header")?;
        let raw_value = header_value.trim();
        let cert = match self.config.certificate_header_format {
            CertHeaderFormat::Base64Encoded => {
                let body: String = raw_value.chars().filter(|ch| !ch.is_ascii_whitespace()).collect();
                let leaf_der = STANDARD.decode(body).map_err(|_| "invalid-base64-cert")?;
                self.verify_leaf_der(&leaf_der)?;
                X509::from_der(&leaf_der).map_err(|_| "invalid-cert-pem")?
            }
            CertHeaderFormat::UrlEncoded => {
                let cert_pem = percent_decode_str(raw_value)
                    .decode_utf8()
                    .map_err(|_| "invalid-url-encoded-cert")?;
                self.verify_against_ca(cert_pem.as_bytes())?;
                X509::from_pem(cert_pem.as_bytes()).map_err(|_| "invalid-cert-pem")?
            }
        };
        Ok(extract_client_cert_info_from_cert(&cert))
    }

    async fn reject(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
        detail: &'static str,
    ) -> PluginRunningResult {
        plugin_log.push_err(detail);
        apply_auth_failure_delay(self.config.auth_failure_delay_ms).await;
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
#[path = "../../../../tests/unit/plugins/http/header_cert_auth/plugin_test.rs"]
mod tests;
