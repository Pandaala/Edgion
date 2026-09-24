//! LDAP Authentication plugin implementation

use crate::plugins::http::common::auth_common::set_sensitive_request_header;
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
        let _ = set_sensitive_request_header(session, &self.auth.credential_identifier_header, username);
        let _ = session.remove_request_header(&self.auth.anonymous_header);
    }

    fn apply_anonymous_headers(&self, session: &mut dyn PluginSession, anonymous_user: &str) {
        let _ = set_sensitive_request_header(session, &self.auth.credential_identifier_header, anonymous_user);
        let _ = set_sensitive_request_header(session, &self.auth.anonymous_header, "true");
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

        let Ok(bind_dn) = self.build_bind_dn(credentials.username()) else {
            plugin_log.push("Deny bad username");
            return self.unauthorized(session).await;
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
#[path = "../../../../tests/unit/plugins/http/ldap_auth/plugin_test.rs"]
mod tests;
