//! LDAP Authentication plugin implementation

use std::ops::Range;
use std::time::Duration;

use crate::observe::metrics::registry::{record_ldap_auth_error, record_outbound_tls_san_mismatch};
use crate::plugins::http::common::auth_common::{
    apply_auth_failure_delay, send_gateway_auth_error_response, send_gateway_plain_error_response,
};
use crate::plugins::http::common::outbound::egress::{
    default_block_private, egress_dns_cache_ttl, EgressPolicy, EgressResolver,
};
use crate::plugins::runtime::log::{la, sensitive};
use crate::plugins::runtime::{DyeOutcome, PluginLog, PluginSession, PluginSessionHeaderExt, RequestFilter};
use crate::tls::validation::outbound_san_mismatch_reason;
use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};
use bytes::Bytes;
use edgion_common::dns::ResolverSpec;
use edgion_common::link_sys_core::outbound_tls::{resolve_outbound_tls, EffectiveCa};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::LdapAuthConfig;
use ldap3::{LdapConnAsync, LdapConnSettings, StdStream};
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use rustls::{ClientConfig, RootCertStore};

const PLUGIN_NAME: &str = "LdapAuth";

/// LDAP Authentication plugin
pub struct LdapAuth {
    ldap_url: String,
    ldap_host: String,
    ldap_port: u16,
    conn_settings: LdapConnSettings,
    egress: EgressResolver,
    timeout: Duration,
    bind_dn_plan: BindDnPlan,
    auth: AuthPolicy,
}

enum BindDnPlan {
    Template(String),
    AttributeBase { attribute: String, base_dn: String },
}

struct AuthPolicy {
    header_type: String,
    hide_credentials: bool,
    auth_failure_delay_ms: u64,
    anonymous: Option<String>,
    realm: String,
    credential_identifier_header: String,
    anonymous_header: String,
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

fn ldap_egress_resolver() -> EgressResolver {
    EgressResolver::new(
        ResolverSpec::System,
        EgressPolicy {
            block_private: default_block_private(),
        },
        egress_dns_cache_ttl(),
    )
}

/// Resolve and classify `host` at dial time, then connect only to admitted
/// addresses. The original hostname is preserved by the caller for LDAP TLS.
async fn connect_admitted_ldap_tcp(
    resolver: &EgressResolver,
    host: &str,
    port: u16,
) -> Result<std::net::TcpStream, LdapAuthError> {
    let addrs = resolver
        .resolve_socket_addrs(host, port)
        .await
        .map_err(|_| LdapAuthError::ConnectionFailed)?;
    dial_admitted_addrs(addrs).await
}

async fn dial_admitted_addrs(addrs: Vec<std::net::SocketAddr>) -> Result<std::net::TcpStream, LdapAuthError> {
    for addr in addrs {
        if let Ok(stream) = tokio::net::TcpStream::connect(addr).await {
            return stream.into_std().map_err(|_| LdapAuthError::ConnectionFailed);
        }
    }
    Err(LdapAuthError::ConnectionFailed)
}

enum ParseAuthResult {
    Missing,
    Invalid,
    Valid(DecodedCredentials),
}

struct DecodedCredentials {
    decoded: String,
    username: Range<usize>,
    password: Range<usize>,
}

impl DecodedCredentials {
    fn username(&self) -> &str {
        &self.decoded[self.username.clone()]
    }

    fn password(&self) -> &str {
        &self.decoded[self.password.clone()]
    }
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
        let timeout = Duration::from_millis(config.timeout);

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

        let mut conn_settings = LdapConnSettings::new()
            .set_conn_timeout(timeout)
            .set_starttls(config.start_tls);
        if let Some(tls_config) = tls_config {
            conn_settings = conn_settings.set_config(tls_config);
        }

        let bind_dn_plan = match &config.bind_dn_template {
            Some(template) => BindDnPlan::Template(template.clone()),
            None => BindDnPlan::AttributeBase {
                attribute: config.attribute.clone(),
                base_dn: config.base_dn.clone(),
            },
        };

        let auth = AuthPolicy {
            header_type: config.header_type.clone(),
            hide_credentials: config.hide_credentials,
            auth_failure_delay_ms: config.auth_failure_delay_ms,
            anonymous: config.anonymous.clone(),
            realm: config.realm.clone(),
            credential_identifier_header: config.credential_identifier_header.clone(),
            anonymous_header: config.anonymous_header.clone(),
        };

        Ok(Self {
            ldap_url,
            ldap_host: config.ldap_host.clone(),
            ldap_port: config.ldap_port,
            conn_settings,
            egress: ldap_egress_resolver(),
            timeout,
            bind_dn_plan,
            auth,
        })
    }

    fn parse_authorization_value(&self, auth_str: &str) -> Option<DecodedCredentials> {
        let trimmed = auth_str.trim();
        let mut parts = trimmed.splitn(2, ' ');
        let scheme = parts.next()?;
        let encoded = parts.next()?.trim();

        if !scheme.eq_ignore_ascii_case(&self.auth.header_type) || encoded.is_empty() {
            return None;
        }

        let decoded = general_purpose::STANDARD.decode(encoded).ok()?;
        let decoded = String::from_utf8(decoded).ok()?;
        let colon = decoded.find(':')?;
        let raw_username = &decoded[..colon];
        let trimmed_username = raw_username.trim();
        let username_start = raw_username.len() - raw_username.trim_start().len();
        let username_end = username_start + trimmed_username.len();
        let password = colon + 1..decoded.len();

        if username_start == username_end || password.is_empty() {
            return None;
        }

        Some(DecodedCredentials {
            decoded,
            username: username_start..username_end,
            password,
        })
    }

    /// Parse credentials from request headers.
    /// Priority: Proxy-Authorization > Authorization.
    fn parse_credentials(&self, session: &dyn PluginSession) -> ParseAuthResult {
        if let Some(value) = session.header_str("proxy-authorization") {
            return self
                .parse_authorization_value(value)
                .map_or(ParseAuthResult::Invalid, ParseAuthResult::Valid);
        }

        match session.header_str("authorization") {
            Some(value) => self
                .parse_authorization_value(value)
                .map_or(ParseAuthResult::Invalid, ParseAuthResult::Valid),
            None => ParseAuthResult::Missing,
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

        match &self.bind_dn_plan {
            BindDnPlan::Template(template) => Ok(template.replace("{username}", escaped.as_ref())),
            BindDnPlan::AttributeBase { attribute, base_dn } => Ok(format!("{}={},{}", attribute, escaped, base_dn)),
        }
    }

    async fn ldap_bind(&self, bind_dn: &str, password: &str) -> Result<(), LdapAuthError> {
        let (conn, mut ldap) = tokio::time::timeout(self.timeout, async {
            let stream = connect_admitted_ldap_tcp(&self.egress, &self.ldap_host, self.ldap_port).await?;
            let settings = self.conn_settings.clone().set_std_stream(StdStream::Tcp(stream));
            LdapConnAsync::with_settings(settings, &self.ldap_url)
                .await
                .map_err(|e| map_ldap_connect_error(&e))
        })
        .await
        .map_err(|_| LdapAuthError::Timeout)??;

        ldap3::drive!(conn);

        let bind_result = tokio::time::timeout(self.timeout, ldap.simple_bind(bind_dn, password))
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
        let _ = session.set_request_header(&self.auth.credential_identifier_header, username);
        let _ = session.remove_request_header(&self.auth.anonymous_header);
    }

    fn apply_anonymous_headers(&self, session: &mut dyn PluginSession, anonymous_user: &str) {
        let _ = session.set_request_header(&self.auth.credential_identifier_header, anonymous_user);
        let _ = session.set_request_header(&self.auth.anonymous_header, "true");
    }

    fn hide_credentials_if_needed(&self, session: &mut dyn PluginSession) {
        if self.auth.hide_credentials {
            let _ = session.remove_request_header("authorization");
            let _ = session.remove_request_header("proxy-authorization");
        }
    }

    async fn unauthorized(&self, session: &mut dyn PluginSession) -> PluginRunningResult {
        let _ = send_gateway_auth_error_response(
            session,
            401,
            &self.auth.header_type,
            &self.auth.realm,
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
        PLUGIN_NAME
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        let credentials = match self.parse_credentials(session) {
            ParseAuthResult::Valid(credentials) => credentials,
            ParseAuthResult::Missing => {
                if let Some(anon_user) = &self.auth.anonymous {
                    self.apply_anonymous_headers(session, anon_user);
                    self.hide_credentials_if_needed(session);
                    plugin_log.push("Allow anonymous");
                    return PluginRunningResult::GoodNext;
                }

                plugin_log.push("Deny no credentials");
                apply_auth_failure_delay(self.auth.auth_failure_delay_ms).await;
                return self.unauthorized(session).await;
            }
            ParseAuthResult::Invalid => {
                plugin_log.push("Deny invalid header");
                apply_auth_failure_delay(self.auth.auth_failure_delay_ms).await;
                return self.unauthorized(session).await;
            }
        };

        let bind_dn = match self.build_bind_dn(credentials.username()) {
            Ok(dn) => dn,
            Err(_) => {
                plugin_log.push("Deny bad username");
                return self.unauthorized(session).await;
            }
        };

        match self.ldap_bind(&bind_dn, credentials.password()).await {
            Ok(()) => {
                self.apply_authenticated_headers(session, credentials.username());
                self.hide_credentials_if_needed(session);
                plugin_log.push_fmt("OK user {}", la![sensitive(credentials.username())]);
                PluginRunningResult::GoodNext
            }
            Err(LdapAuthError::InvalidCredentials) => {
                // Uniform credential-rejection tag, matching basic/key/hmac/jwt_auth.
                // LDAP rc 49 (invalidCredentials) is returned for both a nonexistent DN
                // and a wrong password, so this branch cannot — and must not appear to —
                // discriminate username existence.
                plugin_log.push("Deny bad credential");
                apply_auth_failure_delay(self.auth.auth_failure_delay_ms).await;
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
    use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
    use edgion_resources::resources::common::OutboundTlsLocal;
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;

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
        LdapAuth::new(&test_config()).expect("default plain LDAP must build")
    }

    #[test]
    fn test_parse_authorization_value_invalid() {
        let auth = ldap_auth_default();
        assert!(auth.parse_authorization_value("Basic abc").is_none());
        assert!(auth.parse_authorization_value("ldap !!!").is_none());
        assert!(auth.parse_authorization_value("ldap").is_none());
    }

    #[test]
    fn decoded_credentials_use_one_byte_safe_utf8_backing_buffer() {
        let auth = ldap_auth_default();
        let payload = "  José :pä:ss ";
        let header = format!("LdAp {}", general_purpose::STANDARD.encode(payload));
        let credentials = auth.parse_authorization_value(&header).expect("credentials must parse");

        assert_eq!(credentials.username(), "José");
        assert_eq!(credentials.password(), "pä:ss ");
        let backing = credentials.decoded.as_bytes().as_ptr_range();
        assert!(backing.contains(&credentials.username().as_ptr()));
        assert!(backing.contains(&credentials.password().as_ptr()));
    }

    #[test]
    fn parse_credentials_short_circuits_authorization_when_proxy_is_present() {
        use crate::plugins::runtime::traits::session::MockPluginSession;

        let auth = ldap_auth_default();
        let proxy = format!("ldap {}", general_purpose::STANDARD.encode("alice:password"));
        // A *different* valid credential in `authorization`: the assertion below
        // only holds if `proxy-authorization` short-circuits it. Asserting the
        // winner positively is stronger than asserting `authorization` is never
        // read, because it survives any change to how the header is fetched.
        let fallback = format!("ldap {}", general_purpose::STANDARD.encode("mallory:other"));
        let mut session = MockPluginSession::new();
        setup_header_mocks(
            &mut session,
            &[
                ("proxy-authorization", proxy.as_str()),
                ("authorization", fallback.as_str()),
            ],
        );

        match auth.parse_credentials(&session) {
            ParseAuthResult::Valid(credentials) => {
                assert_eq!(credentials.username(), "alice");
                assert_eq!(credentials.password(), "password");
            }
            ParseAuthResult::Missing | ParseAuthResult::Invalid => panic!("proxy credentials must be valid"),
        }
    }

    #[test]
    fn invalid_proxy_does_not_fall_back_to_authorization() {
        use crate::plugins::runtime::traits::session::MockPluginSession;

        let auth = ldap_auth_default();
        // A malformed proxy header must be terminal. The valid `authorization`
        // below would parse cleanly, so a `Valid` result here would prove a
        // fallback happened.
        let fallback = format!("ldap {}", general_purpose::STANDARD.encode("mallory:other"));
        let mut session = MockPluginSession::new();
        setup_header_mocks(
            &mut session,
            &[
                ("proxy-authorization", "ldap !!!"),
                ("authorization", fallback.as_str()),
            ],
        );

        assert!(matches!(auth.parse_credentials(&session), ParseAuthResult::Invalid));
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
        let auth = LdapAuth::new(&config).expect("must build");
        let dn = auth.build_bind_dn("alice,admin").unwrap();
        assert_eq!(dn, r"uid=alice\2cadmin,ou=people,dc=example,dc=com");
    }

    fn secret_with(entries: &[(&str, &[u8])]) -> Secret {
        let data = entries
            .iter()
            .map(|(name, value)| ((*name).to_string(), ByteString(value.to_vec())))
            .collect();
        Secret {
            data: Some(data),
            type_: Some("kubernetes.io/tls".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn plain_ldap_preserves_caller_secrets_without_retaining_config() {
        let mut config = test_config();
        config.tls = OutboundTlsLocal {
            verify: Some(false),
            ..Default::default()
        };
        config.resolved_ca_certificates = Some(vec![secret_with(&[("ca.crt", b"caller-ca")])]);
        config.resolved_client_certificate = Some(secret_with(&[
            ("tls.crt", b"caller-client-cert"),
            ("tls.key", b"caller-client-key"),
        ]));
        let original_ca = config.resolved_ca_certificates.clone();
        let original_client = config.resolved_client_certificate.clone();

        let auth = LdapAuth::new(&config).expect("plain LDAP must build");
        assert_eq!(auth.ldap_url, "ldap://ldap.example.com:389");
        assert!(!auth.conn_settings.starttls());
        assert_eq!(config.resolved_ca_certificates, original_ca);
        assert_eq!(config.resolved_client_certificate, original_client);
    }

    #[test]
    fn prepared_plain_generations_remain_independent() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<LdapAuth>();

        let k1_config = LdapAuthConfig {
            ldap_host: "ldap-one.example.com".to_string(),
            ldap_port: 1389,
            base_dn: "dc=one,dc=example".to_string(),
            attribute: "uid".to_string(),
            header_type: "ldap-one".to_string(),
            realm: "Realm One".to_string(),
            timeout: 111,
            ..Default::default()
        };
        let k1 = LdapAuth::new(&k1_config).expect("K1 must build");

        let k2_config = LdapAuthConfig {
            ldap_host: "ldap-two.example.com".to_string(),
            ldap_port: 2389,
            base_dn: "dc=unused,dc=example".to_string(),
            attribute: "cn".to_string(),
            bind_dn_template: Some("cn={username},ou=people,dc=two,dc=example".to_string()),
            header_type: "ldap-two".to_string(),
            realm: "Realm Two".to_string(),
            anonymous: Some("guest-two".to_string()),
            hide_credentials: true,
            timeout: 222,
            ..Default::default()
        };
        let k2 = LdapAuth::new(&k2_config).expect("K2 must build");

        assert_eq!(k1.ldap_url, "ldap://ldap-one.example.com:1389");
        assert_eq!(k1.ldap_host, "ldap-one.example.com");
        assert_eq!(k1.ldap_port, 1389);
        assert_eq!(k1.timeout, Duration::from_millis(111));
        assert!(!k1.conn_settings.starttls());
        assert_eq!(k1.auth.header_type, "ldap-one");
        assert_eq!(k1.auth.realm, "Realm One");
        assert!(matches!(k1.bind_dn_plan, BindDnPlan::AttributeBase { .. }));

        assert_eq!(k2.ldap_url, "ldap://ldap-two.example.com:2389");
        assert_eq!(k2.timeout, Duration::from_millis(222));
        assert!(!k2.conn_settings.starttls());
        assert_eq!(k2.auth.header_type, "ldap-two");
        assert_eq!(k2.auth.realm, "Realm Two");
        assert_eq!(k2.auth.anonymous.as_deref(), Some("guest-two"));
        assert!(k2.auth.hide_credentials);
        assert!(matches!(k2.bind_dn_plan, BindDnPlan::Template(_)));

        assert_eq!(k1.ldap_url, "ldap://ldap-one.example.com:1389");
        assert_eq!(k1.auth.realm, "Realm One");
        assert_eq!(k1.build_bind_dn("alice").unwrap(), "uid=alice,dc=one,dc=example");
    }

    #[test]
    fn plugin_name_is_static_across_instances() {
        let first = ldap_auth_default();
        let second = ldap_auth_default();
        assert_eq!(first.name(), PLUGIN_NAME);
        assert_eq!(second.name(), PLUGIN_NAME);
        assert_eq!(first.name().as_ptr(), second.name().as_ptr());
    }

    #[tokio::test]
    async fn anonymous_applies_only_when_both_credential_headers_are_missing() {
        use crate::plugins::runtime::traits::session::MockPluginSession;
        use mockall::predicate::eq;

        let config = LdapAuthConfig {
            anonymous: Some("guest".to_string()),
            ..test_config()
        };
        let auth = LdapAuth::new(&config).expect("anonymous plugin must build");
        let mut missing = MockPluginSession::new();
        setup_header_mocks(&mut missing, &[]);
        missing
            .expect_set_request_header()
            .with(eq("X-Credential-Identifier"), eq("guest"))
            .times(1)
            .returning(|_, _| Ok(()));
        missing
            .expect_set_request_header()
            .with(eq("X-Anonymous-Consumer"), eq("true"))
            .times(1)
            .returning(|_, _| Ok(()));
        let mut missing_log = PluginLog::new(PLUGIN_NAME);
        assert_eq!(
            auth.run_request(&mut missing, &mut missing_log).await,
            PluginRunningResult::GoodNext
        );
        assert!(missing_log.contains("Allow anonymous"));

        let mut invalid = MockPluginSession::new();
        setup_header_mocks(&mut invalid, &[("proxy-authorization", "ldap invalid-base64")]);
        invalid.expect_write_response_header().times(1).returning(|_, _| Ok(()));
        invalid.expect_write_response_body().times(1).returning(|body, end| {
            assert_eq!(body.as_deref(), Some(Bytes::from_static(b"401 Unauthorized").as_ref()));
            assert!(end);
            Ok(())
        });
        invalid.expect_shutdown().times(1).returning(|| {});
        let mut invalid_log = PluginLog::new(PLUGIN_NAME);
        assert_eq!(
            auth.run_request(&mut invalid, &mut invalid_log).await,
            PluginRunningResult::ErrTerminateRequest
        );
        assert!(invalid_log.contains("Deny invalid header"));
        assert!(!invalid_log.contains("Allow anonymous"));
    }

    #[test]
    fn ldap_new_tls_cases_are_process_isolated() {
        const CHILD_ENV: &str = "EDGION_LDAP_AUTH_NEW_TLS_CHILD";
        const TEST_NAME: &str = "ldap_new_tls_cases_are_process_isolated";
        const TEST_MODULE: &str = "plugins::http::ldap_auth::plugin::tests";

        if std::env::var(CHILD_ENV).as_deref() != Ok(TEST_NAME) {
            let exact_test_name = format!("{TEST_MODULE}::{TEST_NAME}");
            let output = std::process::Command::new(
                std::env::current_exe().expect("the Rust test harness must have a current executable"),
            )
            .args(["--exact", exact_test_name.as_str(), "--nocapture"])
            .env(CHILD_ENV, TEST_NAME)
            .output()
            .expect("the isolated LdapAuth TLS constructor test process must start");
            assert!(
                output.status.success(),
                "isolated LdapAuth TLS constructor test failed:\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }

        run_ldap_new_tls_case_table();
    }

    fn run_ldap_new_tls_case_table() {
        use edgion_common::link_sys_core::outbound_tls::set_outbound_global_default;
        use edgion_resources::resources::backend_tls_policy::{
            BackendTLSPolicyCACertificateRef, SubjectAltName, SubjectAltNameType,
        };
        use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
        use edgion_resources::resources::common::{OutboundTlsConfig, SecretObjectReference};

        install_test_crypto_provider();
        set_outbound_global_default(OutboundTlsConfig::default());

        let ldaps = LdapAuth::new(&LdapAuthConfig {
            ldaps: true,
            ldap_port: 636,
            ..test_config()
        })
        .expect("LDAPS with system roots must build");
        assert_eq!(ldaps.ldap_url, "ldaps://ldap.example.com:636");
        assert!(!ldaps.conn_settings.starttls());
        assert!(!ldaps.conn_settings.clone().starttls());

        let starttls = LdapAuth::new(&LdapAuthConfig {
            start_tls: true,
            ..test_config()
        })
        .expect("StartTLS with system roots must build");
        assert_eq!(starttls.ldap_url, "ldap://ldap.example.com:389");
        assert!(starttls.conn_settings.starttls());
        assert!(starttls.conn_settings.clone().starttls());

        let skip_verify = LdapAuth::new(&LdapAuthConfig {
            ldaps: true,
            tls: OutboundTlsLocal {
                verify: Some(false),
                ..Default::default()
            },
            ..test_config()
        })
        .expect("effective verify=false must build the explicit skip-verifier path");
        assert_eq!(skip_verify.ldap_url, "ldaps://ldap.example.com:389");

        let san_override = LdapAuth::new(&LdapAuthConfig {
            ldaps: true,
            tls: OutboundTlsLocal {
                verify: Some(false),
                validation: Some(OutboundTlsValidation {
                    subject_alt_names: Some(vec![SubjectAltName {
                        san_type: SubjectAltNameType::Hostname,
                        hostname: Some("ldap.example.com".to_string()),
                        uri: None,
                    }]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..test_config()
        })
        .expect("SAN policy must override verify=false and retain secure TLS");
        assert_eq!(san_override.ldap_url, "ldaps://ldap.example.com:389");

        let missing_ca = LdapAuth::new(&LdapAuthConfig {
            ldaps: true,
            tls: OutboundTlsLocal {
                validation: Some(OutboundTlsValidation {
                    ca_certificate_refs: Some(vec![BackendTLSPolicyCACertificateRef {
                        kind: "Secret".to_string(),
                        name: "missing-ca".to_string(),
                        group: String::new(),
                        namespace: None,
                    }]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..test_config()
        });
        assert!(missing_ca.is_err(), "unresolved declared CA must fail closed");

        let invalid_ca = LdapAuth::new(&LdapAuthConfig {
            ldaps: true,
            tls: OutboundTlsLocal {
                validation: Some(OutboundTlsValidation {
                    ca_certificate_refs: Some(vec![BackendTLSPolicyCACertificateRef {
                        kind: "Secret".to_string(),
                        name: "invalid-ca".to_string(),
                        group: String::new(),
                        namespace: None,
                    }]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            resolved_ca_certificates: Some(vec![secret_with(&[("ca.crt", b"not a PEM certificate")])]),
            ..test_config()
        });
        assert!(invalid_ca.is_err(), "invalid declared CA must fail construction");

        let client_ref = SecretObjectReference {
            group: None,
            kind: None,
            name: "ldap-client".to_string(),
            namespace: None,
        };
        let missing_client = LdapAuth::new(&LdapAuthConfig {
            ldaps: true,
            tls: OutboundTlsLocal {
                client_certificate_ref: Some(client_ref.clone()),
                ..Default::default()
            },
            ..test_config()
        });
        assert!(
            missing_client.is_err(),
            "unresolved declared client identity must fail closed"
        );

        let invalid_client = LdapAuth::new(&LdapAuthConfig {
            ldaps: true,
            tls: OutboundTlsLocal {
                client_certificate_ref: Some(client_ref.clone()),
                ..Default::default()
            },
            resolved_client_certificate: Some(secret_with(&[
                ("tls.crt", b"invalid certificate"),
                ("tls.key", b"invalid private key"),
            ])),
            ..test_config()
        });
        assert!(
            invalid_client.is_err(),
            "invalid declared client identity must fail construction"
        );

        let generated = rcgen::generate_simple_self_signed(vec!["client.example.com".to_string()])
            .expect("test client identity must generate");
        let cert_pem = generated.cert.pem();
        let key_pem = generated.signing_key.serialize_pem();
        set_outbound_global_default(OutboundTlsConfig {
            client_certificate_ref: Some(client_ref),
            resolved_client_certificate: Some(secret_with(&[
                ("tls.crt", cert_pem.as_bytes()),
                ("tls.key", key_pem.as_bytes()),
            ])),
            ..Default::default()
        });
        let inherited_client = LdapAuth::new(&LdapAuthConfig {
            ldaps: true,
            ..test_config()
        });
        assert!(
            inherited_client.is_ok(),
            "local TLS must inherit the valid global client identity"
        );

        set_outbound_global_default(OutboundTlsConfig {
            verify: false,
            ..Default::default()
        });
        let inherited_skip_verify = LdapAuth::new(&LdapAuthConfig {
            ldaps: true,
            ..test_config()
        });
        assert!(
            inherited_skip_verify.is_ok(),
            "unset local verify must inherit global false"
        );

        set_outbound_global_default(OutboundTlsConfig::default());
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

    #[tokio::test]
    async fn link_local_literal_is_blocked_at_dial_time() {
        let resolver = ldap_egress_resolver();
        let err = connect_admitted_ldap_tcp(&resolver, "169.254.169.254", 389)
            .await
            .expect_err("link-local LDAP targets must fail closed");
        assert!(matches!(err, LdapAuthError::ConnectionFailed));
    }

    #[tokio::test]
    async fn dns_resolution_failure_fails_closed() {
        let resolver = EgressResolver::new(
            ResolverSpec::StandardDns {
                servers: vec!["not-a-dns-server".to_string()],
            },
            EgressPolicy {
                block_private: default_block_private(),
            },
            egress_dns_cache_ttl(),
        );
        let err = connect_admitted_ldap_tcp(&resolver, "ldap.example.com", 389)
            .await
            .expect_err("LDAP DNS failure must fail closed");
        assert!(matches!(err, LdapAuthError::ConnectionFailed));
    }

    #[tokio::test]
    async fn loopback_literal_is_classified_in_unit_tests() {
        let resolver = ldap_egress_resolver();
        let addrs = resolver
            .resolve_socket_addrs("127.0.0.1", 389)
            .await
            .expect("unit tests allow loopback through EgressResolver");
        assert_eq!(addrs, vec!["127.0.0.1:389".parse().unwrap()]);
    }

    #[tokio::test]
    async fn dial_skips_refused_candidate_and_uses_next() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind admitted fallback listener");
        let admitted = listener.local_addr().expect("listener addr");
        let accept = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept fallback connection");
            stream
        });

        let refused = "127.0.0.1:1".parse().unwrap();
        let std_stream = dial_admitted_addrs(vec![refused, admitted])
            .await
            .expect("second admitted address must be dialed");
        drop(std_stream);
        accept.await.expect("accepted fallback connection");
    }

    #[tokio::test]
    async fn all_refused_candidates_fail_closed() {
        let refused = "127.0.0.1:1".parse().unwrap();
        let err = dial_admitted_addrs(vec![refused])
            .await
            .expect_err("no admitted TCP candidate must fail closed");
        assert!(matches!(err, LdapAuthError::ConnectionFailed));
    }
}
