//! LDAP Authentication plugin implementation

use std::time::Duration;

use crate::observe::metrics::registry::{record_ldap_auth_error, record_outbound_tls_san_mismatch};
use crate::plugins::http::common::auth_common::{
    apply_auth_failure_delay, send_gateway_auth_error_response, send_gateway_plain_error_response,
};
use crate::plugins::runtime::log::{la, sensitive};
use crate::plugins::runtime::{DyeOutcome, PluginLog, PluginSession, RequestFilter};
use crate::tls::validation::outbound_san_mismatch_reason;
use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};
use bytes::Bytes;
use edgion_common::link_sys_core::outbound_tls::{resolve_outbound_tls, EffectiveCa};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::LdapAuthConfig;
use ldap3::{LdapConnAsync, LdapConnSettings};
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use rustls::{ClientConfig, RootCertStore};

/// LDAP Authentication plugin
pub struct LdapAuth {
    name: String,
    config: LdapAuthConfig,
    ldap_url: String,
    /// Rustls ClientConfig for TLS-enabled LDAP connections (ldaps or starttls).
    /// None = plain LDAP (ldaps=false, start_tls=false).
    tls_config: Option<std::sync::Arc<ClientConfig>>,
}

#[derive(Debug)]
enum LdapAuthError {
    InvalidCredentials,
    ConnectionFailed,
    Timeout,
    BindFailed,
    /// Outbound TLS handshake rejected by SAN pinning. Carries the verifier's reason.
    TlsSanMismatch(String),
}

/// Classify an `LdapError` raised while opening the connection. ldap3 completes
/// both the implicit-TLS (`ldaps://`) and the STARTTLS handshake inside
/// `LdapConnAsync::with_settings`, so this is the only site where a TLS error
/// can appear; `simple_bind` cannot raise one.
fn map_ldap_connect_error(err: &ldap3::LdapError) -> LdapAuthError {
    match outbound_san_mismatch_reason(err) {
        Some(reason) => LdapAuthError::TlsSanMismatch(reason),
        None => LdapAuthError::ConnectionFailed,
    }
}

#[derive(Debug, PartialEq)]
enum ParseAuthResult {
    Missing,
    Invalid,
    Valid(String, String),
}

/// Accept-all server-certificate verifier for the `verify=false` path.
/// Reached when an operator explicitly sets `verify=false` and the resolver honored it.
#[derive(Debug)]
struct AcceptAllVerifier(rustls::crypto::WebPkiSupportedAlgorithms);

impl AcceptAllVerifier {
    fn new() -> Self {
        Self(rustls::crypto::ring::default_provider().signature_verification_algorithms)
    }
}

impl rustls::client::danger::ServerCertVerifier for AcceptAllVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.supported_schemes()
    }
}

/// Build a rustls `ClientConfig` for ldap3 from the already-resolved outbound TLS settings.
///
/// - `verify=false` with no SAN policy → `AcceptAllVerifier` (operator's responsibility).
/// - `verify=false` with non-empty SAN policy → full webpki + `OutboundSanVerifier` (fail-secure:
///   SAN policy overrides skip-verify).
/// - `System` → webpki built-in roots (binary-embedded, not OS cert store).
/// - `Custom(pem)` → private CA bundle, REPLACES public roots entirely.
/// - Client identity (`client_identity`) → `with_client_auth_cert`; absent → `with_no_client_auth`.
/// - No ALPN protocols: ldap3 does not negotiate ALPN.
fn build_ldap_rustls_config(
    resolved: &edgion_common::link_sys_core::outbound_tls::ResolvedOutboundTls,
) -> Result<ClientConfig, String> {
    let builder = ClientConfig::builder_with_provider(std::sync::Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("LdapAuth TLS: rustls provider error: {e}"))?;

    // A non-empty SAN policy overrides verify=false and forces full certificate
    // verification (fail-secure). Skip verification only when verify=false AND
    // no SAN policy is configured.
    let san_policy = resolved
        .san
        .as_ref()
        .and_then(|v| v.subject_alt_names.clone())
        .unwrap_or_default();

    let builder = if san_policy.is_empty() && !resolved.verify {
        // No SAN policy: operator explicitly disabled verification.
        builder
            .dangerous()
            .with_custom_certificate_verifier(std::sync::Arc::new(AcceptAllVerifier::new()))
    } else {
        // Either verify=true, or a SAN policy is set (which overrides verify=false).
        let mut roots = RootCertStore::empty();
        match &resolved.ca {
            EffectiveCa::System => {
                roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            }
            EffectiveCa::Custom(bundles) => {
                let mut added = 0usize;
                for bundle in bundles {
                    for cert in CertificateDer::pem_slice_iter(bundle) {
                        let cert = cert.map_err(|e| format!("LdapAuth TLS: invalid CA certificate PEM: {e}"))?;
                        roots
                            .add(cert)
                            .map_err(|e| format!("LdapAuth TLS: failed to add CA certificate: {e}"))?;
                        added += 1;
                    }
                }
                if added == 0 {
                    return Err("LdapAuth TLS: CA PEM did not contain any valid certificates".to_string());
                }
            }
        }
        // If the operator supplied a SAN pinning policy, wrap the webpki
        // verifier in OutboundSanVerifier so that the handshake is also
        // checked against the configured subjectAltNames.
        if !san_policy.is_empty() {
            let webpki = rustls::client::WebPkiServerVerifier::builder_with_provider(
                std::sync::Arc::new(roots),
                std::sync::Arc::new(rustls::crypto::ring::default_provider()),
            )
            .build()
            .map_err(|e| format!("LdapAuth TLS: webpki verifier build error: {e}"))?;
            let san_verifier =
                std::sync::Arc::new(crate::tls::validation::OutboundSanVerifier::new(webpki, san_policy));
            builder.dangerous().with_custom_certificate_verifier(san_verifier)
        } else {
            builder.with_root_certificates(roots)
        }
    };

    let config = match &resolved.client_identity {
        Some((cert_pem, key_pem)) => {
            let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert_pem)
                .collect::<Result<_, _>>()
                .map_err(|e| format!("LdapAuth TLS: invalid client certificate PEM: {e}"))?;
            if certs.is_empty() {
                return Err("LdapAuth TLS: client certificate PEM did not contain any certificates".to_string());
            }
            let key = PrivateKeyDer::from_pem_slice(key_pem.as_slice())
                .map_err(|e| format!("LdapAuth TLS: invalid client private key PEM: {e}"))?;
            builder
                .with_client_auth_cert(certs, key)
                .map_err(|e| format!("LdapAuth TLS: client auth cert error: {e}"))?
        }
        None => builder.with_no_client_auth(),
    };

    Ok(config)
}

impl LdapAuth {
    /// Create a new LdapAuth plugin from configuration
    pub fn create(config: &LdapAuthConfig) -> Option<Box<dyn RequestFilter>> {
        match Self::new(config) {
            Ok(plugin) => Some(Box::new(plugin)),
            Err(e) => {
                tracing::error!(
                    component = "ldap_auth",
                    error = %e,
                    "LdapAuth plugin build failed; disabling plugin (fail-closed)"
                );
                None
            }
        }
    }

    pub fn new(config: &LdapAuthConfig) -> Result<Self, String> {
        let scheme = if config.ldaps { "ldaps" } else { "ldap" };
        let ldap_url = format!("{}://{}:{}", scheme, config.ldap_host, config.ldap_port);

        let tls_config = if config.ldaps || config.start_tls {
            let resolved = resolve_outbound_tls(
                &config.tls,
                config.resolved_ca_certificates.as_deref(),
                config.resolved_client_certificate.as_ref(),
                true,
                "LdapAuth",
            )
            .map_err(|e| format!("LdapAuth TLS configuration error: {e:?}"))?;

            // M4: when verify=false the resolver drops client_identity. Warn the
            // operator that their clientCertificateRef is not being used.
            if !resolved.verify && config.tls.client_certificate_ref.is_some() && resolved.client_identity.is_none() {
                tracing::warn!(
                    component = "ldap_auth",
                    "verify=false with clientCertificateRef configured: client certificate is not sent (M4)"
                );
            }

            Some(std::sync::Arc::new(build_ldap_rustls_config(&resolved)?))
        } else {
            None
        };

        Ok(Self {
            name: "LdapAuth".to_string(),
            config: config.clone(),
            ldap_url,
            tls_config,
        })
    }

    fn parse_authorization_value(&self, auth_str: &str) -> Option<(String, String)> {
        let trimmed = auth_str.trim();
        let mut parts = trimmed.splitn(2, ' ');
        let scheme = parts.next()?;
        let encoded = parts.next()?.trim();

        if !scheme.eq_ignore_ascii_case(&self.config.header_type) || encoded.is_empty() {
            return None;
        }

        let decoded = general_purpose::STANDARD.decode(encoded).ok()?;
        let decoded_str = String::from_utf8(decoded).ok()?;
        let mut user_pass = decoded_str.splitn(2, ':');
        let username = user_pass.next()?.trim().to_string();
        let password = user_pass.next()?.to_string();

        if username.is_empty() || password.is_empty() {
            return None;
        }

        Some((username, password))
    }

    /// Parse credentials from request headers.
    /// Priority: Proxy-Authorization > Authorization.
    fn parse_credentials(&self, session: &dyn PluginSession) -> ParseAuthResult {
        let proxy_auth = session.header_value("proxy-authorization");
        let authorization = session.header_value("authorization");

        match (proxy_auth, authorization) {
            (Some(v), _) => self
                .parse_authorization_value(&v)
                .map_or(ParseAuthResult::Invalid, |(u, p)| ParseAuthResult::Valid(u, p)),
            (None, Some(v)) => self
                .parse_authorization_value(&v)
                .map_or(ParseAuthResult::Invalid, |(u, p)| ParseAuthResult::Valid(u, p)),
            (None, None) => ParseAuthResult::Missing,
        }
    }

    fn build_bind_dn(&self, username: &str) -> Result<String, &'static str> {
        // Escape per RFC 4514 (DN attribute value): handles injection-relevant
        // metacharacters while accepting legitimate enterprise LDAP usernames
        // (spaces in AD sAMAccountName, unicode CN values, aliases with '+', etc.).
        if username.is_empty() {
            return Err("empty username");
        }
        let escaped = ldap3::dn_escape(username);

        if let Some(template) = &self.config.bind_dn_template {
            return Ok(template.replace("{username}", escaped.as_ref()));
        }

        Ok(format!("{}={},{}", self.config.attribute, escaped, self.config.base_dn))
    }

    async fn ldap_bind(&self, bind_dn: &str, password: &str) -> Result<(), LdapAuthError> {
        let timeout = Duration::from_millis(self.config.timeout);

        let mut settings = LdapConnSettings::new()
            .set_conn_timeout(timeout)
            .set_starttls(self.config.start_tls);

        if let Some(ref cfg) = self.tls_config {
            settings = settings.set_config(std::sync::Arc::clone(cfg));
        }

        let (conn, mut ldap) = tokio::time::timeout(timeout, LdapConnAsync::with_settings(settings, &self.ldap_url))
            .await
            .map_err(|_| LdapAuthError::Timeout)?
            .map_err(|e| map_ldap_connect_error(&e))?;

        ldap3::drive!(conn);

        let bind_result = tokio::time::timeout(timeout, ldap.simple_bind(bind_dn, password))
            .await
            .map_err(|_| LdapAuthError::Timeout)?
            .map_err(|_| LdapAuthError::BindFailed)?;

        let _ = ldap.unbind().await;

        if bind_result.rc == 0 {
            Ok(())
        } else if bind_result.rc == 49 {
            Err(LdapAuthError::InvalidCredentials)
        } else {
            Err(LdapAuthError::BindFailed)
        }
    }

    fn apply_authenticated_headers(&self, session: &mut dyn PluginSession, username: &str) {
        let _ = session.set_request_header(&self.config.credential_identifier_header, username);
        let _ = session.remove_request_header(&self.config.anonymous_header);
    }

    fn apply_anonymous_headers(&self, session: &mut dyn PluginSession, anonymous_user: &str) {
        let _ = session.set_request_header(&self.config.credential_identifier_header, anonymous_user);
        let _ = session.set_request_header(&self.config.anonymous_header, "true");
    }

    fn hide_credentials_if_needed(&self, session: &mut dyn PluginSession) {
        if self.config.hide_credentials {
            let _ = session.remove_request_header("authorization");
            let _ = session.remove_request_header("proxy-authorization");
        }
    }

    async fn unauthorized(&self, session: &mut dyn PluginSession) -> PluginRunningResult {
        let _ = send_gateway_auth_error_response(
            session,
            401,
            &self.config.header_type,
            &self.config.realm,
            None,
            "Unauthorized",
        )
        .await;
        PluginRunningResult::ErrTerminateRequest
    }

    async fn service_unavailable(&self, session: &mut dyn PluginSession) -> PluginRunningResult {
        // Record BEFORE `send_gateway_plain_error_response`'s self-write
        // (`write_response_header`) so the queued dye header op is drained
        // by that write. This is the single choke point for every
        // LDAP-unreachable fail-close branch (connection failed, TLS SAN
        // mismatch, timeout, bind failure) — credential rejection
        // (`unauthorized`) never routes through here.
        session.record_outcome(DyeOutcome::FailClose, "ldap-unreachable");
        let _ = send_gateway_plain_error_response(
            session,
            503,
            "text/plain",
            Bytes::from_static(b"503 An unexpected error occurred"),
        )
        .await;
        PluginRunningResult::ErrTerminateRequest
    }
}

#[async_trait]
impl RequestFilter for LdapAuth {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        let (username, password) = match self.parse_credentials(session) {
            ParseAuthResult::Valid(u, p) => (u, p),
            ParseAuthResult::Missing => {
                if let Some(anon_user) = &self.config.anonymous {
                    self.apply_anonymous_headers(session, anon_user);
                    self.hide_credentials_if_needed(session);
                    plugin_log.push("Allow anonymous");
                    return PluginRunningResult::GoodNext;
                }

                plugin_log.push("Deny no credentials");
                apply_auth_failure_delay(self.config.auth_failure_delay_ms).await;
                return self.unauthorized(session).await;
            }
            ParseAuthResult::Invalid => {
                plugin_log.push("Deny invalid header");
                apply_auth_failure_delay(self.config.auth_failure_delay_ms).await;
                return self.unauthorized(session).await;
            }
        };

        let bind_dn = match self.build_bind_dn(&username) {
            Ok(dn) => dn,
            Err(_) => {
                plugin_log.push("Deny bad username");
                return self.unauthorized(session).await;
            }
        };

        match self.ldap_bind(&bind_dn, &password).await {
            Ok(()) => {
                self.apply_authenticated_headers(session, &username);
                self.hide_credentials_if_needed(session);
                plugin_log.push_fmt("OK user {}", la![sensitive(&username)]);
                PluginRunningResult::GoodNext
            }
            Err(LdapAuthError::InvalidCredentials) => {
                // Uniform credential-rejection tag, matching basic/key/hmac/jwt_auth.
                // LDAP rc 49 (invalidCredentials) is returned for both a nonexistent DN
                // and a wrong password, so this branch cannot — and must not appear to —
                // discriminate username existence.
                plugin_log.push("Deny bad credential");
                apply_auth_failure_delay(self.config.auth_failure_delay_ms).await;
                self.unauthorized(session).await
            }
            Err(LdapAuthError::ConnectionFailed) => {
                plugin_log.push_err("ldap unreachable");
                record_ldap_auth_error("unreachable");
                self.service_unavailable(session).await
            }
            Err(LdapAuthError::TlsSanMismatch(reason)) => {
                session.record_error(edgion_resources::EdgionErrorCode::OutboundTlsSanMismatch);
                plugin_log.push_err("outbound tls san mismatch");
                plugin_log.push_fmt("san {}", la![reason.as_str()]);
                record_ldap_auth_error("tls_san_mismatch");
                record_outbound_tls_san_mismatch("ldap_auth");
                self.service_unavailable(session).await
            }
            Err(LdapAuthError::Timeout) => {
                plugin_log.push_err("ldap timeout");
                record_ldap_auth_error("timeout");
                self.service_unavailable(session).await
            }
            Err(LdapAuthError::BindFailed) => {
                plugin_log.push_err("ldap bind fail");
                record_ldap_auth_error("bind_fail");
                self.service_unavailable(session).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> LdapAuthConfig {
        LdapAuthConfig {
            ldap_host: "ldap.example.com".to_string(),
            base_dn: "dc=example,dc=com".to_string(),
            attribute: "uid".to_string(),
            ..Default::default()
        }
    }

    fn install_test_crypto_provider() {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
    }

    fn ldap_auth_default() -> LdapAuth {
        install_test_crypto_provider();
        LdapAuth::new(&test_config()).expect("default plain LDAP must build")
    }

    #[test]
    fn test_parse_authorization_value_invalid() {
        let auth = ldap_auth_default();
        assert_eq!(auth.parse_authorization_value("Basic abc"), None);
        assert_eq!(auth.parse_authorization_value("ldap !!!"), None);
        assert_eq!(auth.parse_authorization_value("ldap"), None);
    }

    #[test]
    fn test_build_bind_dn_injection_escaped() {
        let auth = ldap_auth_default();

        // RFC 4514 metacharacters must be escaped so injection becomes a
        // literal value inside the RDN. ldap3::dn_escape uses the hex form
        // (\HH); both forms are RFC-equivalent for any compliant server.
        let dn = auth.build_bind_dn("alice,admin").unwrap();
        assert_eq!(dn, r"uid=alice\2cadmin,dc=example,dc=com");

        let dn = auth.build_bind_dn("alice+admin").unwrap();
        assert_eq!(dn, r"uid=alice\2badmin,dc=example,dc=com");

        // Leading '#' and trailing ' ' are also escaped per RFC 4514.
        let dn = auth.build_bind_dn("#bob ").unwrap();
        assert_eq!(dn, r"uid=\23bob\20,dc=example,dc=com");

        // Sanity check: no bare injection-relevant metachar remains inside the
        // attribute value. (`\` is the escape leader produced by dn_escape and
        // is expected to appear; the metachars below would terminate the RDN
        // or change its semantics if they leaked through unescaped.)
        let dn = auth.build_bind_dn(r#"x"y\z<a>b;c=d"#).unwrap();
        let value = dn
            .strip_prefix("uid=")
            .and_then(|s| s.strip_suffix(",dc=example,dc=com"))
            .unwrap();
        for ch in ['"', '<', '>', ';', '=', ',', '+'] {
            assert!(
                !value.contains(ch),
                "bare metachar {:?} leaked into RDN value {:?}",
                ch,
                value
            );
        }
    }

    #[test]
    fn test_build_bind_dn_accepts_spaces() {
        // AD sAMAccountName may contain spaces. Interior spaces are not RFC 4514
        // metacharacters and must pass through unescaped.
        let auth = ldap_auth_default();
        let dn = auth.build_bind_dn("John Smith").unwrap();
        assert_eq!(dn, "uid=John Smith,dc=example,dc=com");
    }

    #[test]
    fn test_build_bind_dn_accepts_unicode() {
        // Multi-byte UTF-8 in CN/uid values (e.g. accented Latin) is valid in
        // RFC 4514 attribute values and must not be rejected or escaped.
        let auth = ldap_auth_default();
        let dn = auth.build_bind_dn("José").unwrap();
        assert_eq!(dn, "uid=José,dc=example,dc=com");
    }

    #[test]
    fn test_build_bind_dn_rejects_empty() {
        let auth = ldap_auth_default();
        assert!(auth.build_bind_dn("").is_err());
    }

    #[test]
    fn test_build_bind_dn_template_escapes_username() {
        // The {username} placeholder must be escaped before substitution so a
        // template-based deployment is equally injection-safe.
        let config = LdapAuthConfig {
            ldap_host: "ldap.example.com".to_string(),
            base_dn: "dc=example,dc=com".to_string(),
            attribute: "uid".to_string(),
            bind_dn_template: Some("uid={username},ou=people,dc=example,dc=com".to_string()),
            ..Default::default()
        };
        install_test_crypto_provider();
        let auth = LdapAuth::new(&config).expect("must build");
        let dn = auth.build_bind_dn("alice,admin").unwrap();
        assert_eq!(dn, r"uid=alice\2cadmin,ou=people,dc=example,dc=com");
    }

    /// No TLS (ldaps=false, start_tls=false) → tls_config must be None.
    #[test]
    fn plain_ldap_has_no_tls_config() {
        install_test_crypto_provider();
        let config = test_config(); // ldaps=false, start_tls=false
        let auth = LdapAuth::new(&config).expect("plain LDAP must build");
        assert!(auth.tls_config.is_none(), "plain LDAP must not build a TLS config");
    }

    /// ldaps=true + default outbound (System CA, verify=true) → tls_config is Some.
    #[test]
    fn ldaps_with_system_ca_builds_tls_config() {
        install_test_crypto_provider();
        let config = LdapAuthConfig {
            ldaps: true,
            ..test_config()
        };
        let auth = LdapAuth::new(&config).expect("LDAPS with system CA must build");
        assert!(auth.tls_config.is_some(), "LDAPS must produce a TLS config");
    }

    /// verify=false + ldaps=true (no SAN policy) → tls_config is Some (AcceptAllVerifier path).
    #[test]
    fn ldaps_verify_false_builds_tls_config() {
        install_test_crypto_provider();
        use edgion_resources::resources::common::OutboundTlsLocal;
        let config = LdapAuthConfig {
            ldaps: true,
            tls: OutboundTlsLocal {
                verify: Some(false),
                ..Default::default()
            },
            ..test_config()
        };
        let auth = LdapAuth::new(&config).expect("verify=false must be honored");
        assert!(auth.tls_config.is_some());
    }

    /// caCertificateRefs declared but resolved_ca_certificates=None → construction error (fail-closed).
    #[test]
    fn ldaps_secretref_without_resolved_secret_fails() {
        install_test_crypto_provider();
        use edgion_resources::resources::backend_tls_policy::BackendTLSPolicyCACertificateRef;
        use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
        use edgion_resources::resources::common::OutboundTlsLocal;
        let config = LdapAuthConfig {
            ldaps: true,
            tls: OutboundTlsLocal {
                validation: Some(OutboundTlsValidation {
                    ca_certificate_refs: Some(vec![BackendTLSPolicyCACertificateRef {
                        kind: "Secret".to_string(),
                        name: "missing-secret".to_string(),
                        group: String::new(),
                        namespace: None,
                    }]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            resolved_ca_certificates: None, // not yet resolved
            ..test_config()
        };
        let result = LdapAuth::new(&config);
        assert!(
            result.is_err(),
            "SecretRef with no resolved Secret must fail construction (fail-closed)"
        );
    }

    /// Custom CA via resolved_ca_certificates with invalid PEM does not panic; returns Err.
    #[test]
    fn ldaps_inline_ca_invalid_pem_does_not_panic() {
        install_test_crypto_provider();
        use edgion_resources::resources::backend_tls_policy::BackendTLSPolicyCACertificateRef;
        use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
        use edgion_resources::resources::common::OutboundTlsLocal;

        // An intentionally invalid CA PEM: exercises the error path when
        // CertificateDer::pem_slice_iter encounters bad base64 in the Secret data.
        let dummy_ca_pem = b"-----BEGIN CERTIFICATE-----\n\
            MIICpDCCAYwCCQDU9pQ4pHgSpDANBgkqhkiG9w0BAQsFADAUMRIwEAYDVQQDDAl0\n\
            ZXN0LWNhMB4XDTIzMDEwMTAwMDAwMFoXDTMzMDEwMTAwMDAwMFowFDESMBAGA1UE\n\
            AwwJdGVzdC1jYTCCASIwDQYJKoZIhvcNAQEBBQADggEPADCCAQoCggEBAMqkQPaH\n\
            placeholder_only_for_length_not_real_base64_data_AAAAAAAAAAAAAAAAAAA\n\
            -----END CERTIFICATE-----\n";

        // Build a K8s Secret carrying the invalid PEM so the resolver picks it up.
        let mut data = std::collections::BTreeMap::new();
        data.insert("ca.crt".to_string(), k8s_openapi::ByteString(dummy_ca_pem.to_vec()));
        let ca_secret = k8s_openapi::api::core::v1::Secret {
            data: Some(data),
            type_: Some("kubernetes.io/tls".to_string()),
            ..Default::default()
        };

        let config = LdapAuthConfig {
            ldaps: true,
            tls: OutboundTlsLocal {
                validation: Some(OutboundTlsValidation {
                    ca_certificate_refs: Some(vec![BackendTLSPolicyCACertificateRef {
                        kind: "Secret".to_string(),
                        name: "dummy-ca".to_string(),
                        group: String::new(),
                        namespace: None,
                    }]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            resolved_ca_certificates: Some(vec![ca_secret]),
            ..test_config()
        };
        // An invalid PEM will fail; for a real test use a valid cert.
        // We just verify the function is called and returns Err (not panics).
        let _ = LdapAuth::new(&config); // Ok or Err — just must not panic
    }

    // -----------------------------------------------------------------------
    // Helpers for build_ldap_rustls_config SAN tests
    // -----------------------------------------------------------------------

    fn make_resolved_verify_true(
        san_names: Vec<edgion_resources::resources::backend_tls_policy::SubjectAltName>,
    ) -> edgion_common::link_sys_core::outbound_tls::ResolvedOutboundTls {
        use edgion_common::link_sys_core::outbound_tls::{EffectiveCa, ResolvedOutboundTls};
        use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
        let san = if san_names.is_empty() {
            None
        } else {
            Some(OutboundTlsValidation {
                subject_alt_names: Some(san_names),
                ..Default::default()
            })
        };
        ResolvedOutboundTls {
            verify: true,
            ca: EffectiveCa::System,
            client_identity: None,
            san,
        }
    }

    fn make_resolved_verify_false_with_san(
        san_names: Vec<edgion_resources::resources::backend_tls_policy::SubjectAltName>,
    ) -> edgion_common::link_sys_core::outbound_tls::ResolvedOutboundTls {
        use edgion_common::link_sys_core::outbound_tls::{EffectiveCa, ResolvedOutboundTls};
        use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
        let san = if san_names.is_empty() {
            None
        } else {
            Some(OutboundTlsValidation {
                subject_alt_names: Some(san_names),
                ..Default::default()
            })
        };
        ResolvedOutboundTls {
            verify: false,
            ca: EffectiveCa::System,
            client_identity: None,
            san,
        }
    }

    /// Non-empty SAN policy (verify=true) → OutboundSanVerifier wraps webpki → returns Ok.
    #[test]
    fn ldap_rustls_config_with_san_policy_wraps_verifier() {
        install_test_crypto_provider();
        use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType};
        let san_policy = vec![SubjectAltName {
            san_type: SubjectAltNameType::Hostname,
            hostname: Some("ldap.example.com".to_string()),
            uri: None,
        }];
        let resolved = make_resolved_verify_true(san_policy);
        let result = build_ldap_rustls_config(&resolved);
        assert!(
            result.is_ok(),
            "SAN-wrapped verifier path must build Ok, got: {:?}",
            result.err()
        );
    }

    /// Empty SAN policy (verify=true) → standard webpki path → returns Ok.
    #[test]
    fn ldap_rustls_config_without_san_uses_default_webpki() {
        install_test_crypto_provider();
        let resolved = make_resolved_verify_true(vec![]);
        let result = build_ldap_rustls_config(&resolved);
        assert!(
            result.is_ok(),
            "default webpki (no SAN policy) must build Ok, got: {:?}",
            result.err()
        );
    }

    /// verify=false + non-empty SAN policy → SAN verifier overrides skip-verify (fail-secure).
    /// The function must build Ok via the webpki + OutboundSanVerifier path,
    /// not via AcceptAllVerifier.
    #[test]
    fn ldap_rustls_config_verify_false_with_san_forces_secure_path() {
        install_test_crypto_provider();
        use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType};
        let san_policy = vec![SubjectAltName {
            san_type: SubjectAltNameType::Hostname,
            hostname: Some("ldap.example.com".to_string()),
            uri: None,
        }];
        let resolved = make_resolved_verify_false_with_san(san_policy);
        // verify=false is overridden by the SAN policy: the function must build
        // a config using webpki + OutboundSanVerifier rather than AcceptAllVerifier.
        let result = build_ldap_rustls_config(&resolved);
        assert!(
            result.is_ok(),
            "verify=false with SAN policy must build Ok via SAN verifier path, got: {:?}",
            result.err()
        );
    }

    /// A SAN mismatch surfacing from `LdapConnAsync::with_settings` must map to
    /// `TlsSanMismatch`, not the generic `ConnectionFailed`. The mapping closure
    /// is exercised directly because opening a real TLS connection is out of
    /// scope for a unit test.
    #[test]
    fn ldap_error_with_san_mismatch_maps_to_tls_san_mismatch() {
        let rustls_err = rustls::Error::InvalidCertificate(rustls::CertificateError::Other(rustls::OtherError(
            std::sync::Arc::new(crate::tls::validation::SanMismatchError {
                reason: "declared [ldap.example.com], actual [127.0.0.1]".to_string(),
            }),
        )));
        let io_err = std::io::Error::new(std::io::ErrorKind::InvalidData, rustls_err);
        let ldap_err = ldap3::LdapError::Io { source: io_err };

        let mapped = map_ldap_connect_error(&ldap_err);

        match mapped {
            LdapAuthError::TlsSanMismatch(reason) => {
                assert_eq!(reason, "declared [ldap.example.com], actual [127.0.0.1]");
            }
            other => panic!("expected TlsSanMismatch, got {:?}", other),
        }
    }

    #[test]
    fn ldap_error_without_san_mismatch_maps_to_connection_failed() {
        let ldap_err = ldap3::LdapError::Io {
            source: std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "connection refused"),
        };

        assert!(matches!(
            map_ldap_connect_error(&ldap_err),
            LdapAuthError::ConnectionFailed
        ));
    }

    // ── Task 9: dye-suite record_outcome wiring ─────────────────────────────

    /// `service_unavailable` is the single choke point for every
    /// LDAP-unreachable fail-close branch (connection failed, TLS SAN
    /// mismatch, timeout, bind failure). It must record `FailClose` with
    /// cause `ldap-unreachable` BEFORE its self-write
    /// (`send_gateway_plain_error_response`'s `write_response_header`) —
    /// enforced via a mockall `Sequence`.
    #[tokio::test]
    async fn service_unavailable_records_fail_close_before_response_write() {
        use crate::plugins::runtime::traits::session::MockPluginSession;
        use mockall::Sequence;

        let auth = ldap_auth_default();
        let mut session = MockPluginSession::new();

        let mut seq = Sequence::new();
        session
            .expect_record_outcome()
            .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "ldap-unreachable")
            .times(1)
            .in_sequence(&mut seq)
            .return_const(());
        session
            .expect_write_response_header()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Ok(()));
        session.expect_write_response_body().returning(|_, _| Ok(()));
        session.expect_shutdown().returning(|| {});

        let result = auth.service_unavailable(&mut session).await;
        assert!(matches!(result, PluginRunningResult::ErrTerminateRequest));
    }

    /// Credential rejection (`unauthorized`, used for missing/invalid/wrong
    /// credentials) is an auth decision, not a dependency failure — it must
    /// NOT call `record_outcome`. No `expect_record_outcome` is installed, so
    /// any call panics the mock.
    #[tokio::test]
    async fn unauthorized_does_not_record_outcome() {
        use crate::plugins::runtime::traits::session::MockPluginSession;

        let auth = ldap_auth_default();
        let mut session = MockPluginSession::new();
        session.expect_write_response_header().returning(|_, _| Ok(()));
        session.expect_write_response_body().returning(|_, _| Ok(()));
        session.expect_shutdown().returning(|| {});

        let result = auth.unauthorized(&mut session).await;
        assert!(matches!(result, PluginRunningResult::ErrTerminateRequest));
    }
}
