//! LDAP Authentication plugin implementation

use crate::plugins::http::common::auth_common::set_sensitive_request_header;
use std::future::Future;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use crate::observe::metrics::registry::{record_ldap_auth_error, record_outbound_tls_san_mismatch};
use crate::plugins::http::common::auth_common::{
    apply_auth_failure_delay, send_gateway_auth_error_response, send_gateway_plain_error_response,
    PreparedAuthChallenge,
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
use edgion_common::link_sys_core::outbound_tls::{
    resolve_validated_outbound_tls, validate_active_tls_inheritance, validate_local_tls_selection, EffectiveCa,
    GlobalOutboundTlsSnapshot, ValidatedLocalTlsSelection,
};
use edgion_ldap::filter::SearchFilter;
use edgion_ldap::{BindSession, Scope};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::EdgionPlugin;
use edgion_resources::resources::refs::plugin_fields::BoundPluginEntry;
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

const PLUGIN_NAME: &str = "LdapAuth";

/// LDAP Authentication plugin
pub struct LdapAuth {
    ldap_host: String,
    ldap_port: u16,
    transport: LdapTransport,
    egress: EgressResolver,
    timeout: Duration,
    bind_dn_plan: BindDnPlan,
    auth: AuthPolicy,
    /// Unit tests inject an in-memory peer here instead of dialing.
    #[cfg(test)]
    scripted_stream: parking_lot::Mutex<Option<tokio::io::DuplexStream>>,
}

/// How the bind connection is secured. Built once per plugin generation.
enum LdapTransport {
    Plain,
    /// Implicit TLS (`ldaps`).
    Ldaps {
        connector: TlsConnector,
        server_name: Option<ServerName<'static>>,
    },
    /// StartTLS extended operation on a plain connection, then TLS.
    StartTls {
        connector: TlsConnector,
        server_name: Option<ServerName<'static>>,
    },
}

enum BindDnPlan {
    Template(String),
    AttributeBase {
        attribute: String,
        base_dn: String,
    },
    /// Search mode: service bind, single-entry search, user bind.
    Search(SearchPlan),
}

/// Search-mode settings built once per plugin generation.
struct SearchPlan {
    bind_dn: String,
    /// `bind_dn` normalized once for the service-account guard.
    service_dn: edgion_ldap::dn::NormalizedDn,
    /// Resolved service-account password; `None` when the Secret is unavailable.
    password: Option<String>,
    base: String,
    filter: SearchFilter,
    /// Identity attribute requested in the search and forwarded upstream.
    attribute: String,
    time_limit_secs: u32,
    /// Pre-encoded group requirement, checked after a successful user bind.
    groups: Option<SearchFilter>,
}

/// Longest username accepted in search mode (bounds the outbound request).
const MAX_SEARCH_USERNAME_LEN: usize = 1024;

/// What phase 2 of an exchange does on the established session.
enum Exchange<'a> {
    /// Bind as `dn` with the client's password.
    Bind { dn: &'a str, password: &'a str },
    /// Bind as the service account, find the user's entry, bind as it and,
    /// with group requirements, re-bind as the service account and check them.
    Search {
        plan: &'a SearchPlan,
        service_password: &'a str,
        username: &'a str,
        password: &'a str,
    },
}

struct AuthPolicy {
    header_type: String,
    hide_credentials: bool,
    auth_failure_delay_ms: u64,
    anonymous: Option<String>,
    auth_challenge: PreparedAuthChallenge,
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
    /// Search mode: the directory rejected the service-account credentials (rc 49).
    ServiceBindRejected,
    /// Search mode: the service bind or the search failed for another reason.
    SearchFailed,
    /// Search mode: the password was correct but the user is not in the
    /// required groups.
    NotInRequiredGroup,
}

/// Classify a TLS handshake error. `bind_over` performs both the implicit-TLS
/// (`ldaps`) and the STARTTLS handshake through `tokio-rustls`, which reports
/// failures as `std::io::Error`. SAN verification runs only inside the
/// handshake, so `simple_bind` never raises a SAN mismatch; a post-handshake
/// TLS alert (for example a TLS 1.3 client-certificate rejection) surfaces as a
/// bind stream error and maps to `BindFailed`.
fn map_ldap_connect_error(err: &std::io::Error) -> LdapAuthError {
    match outbound_san_mismatch_reason(err) {
        Some(reason) => LdapAuthError::TlsSanMismatch(reason.into_owned()),
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
) -> Result<TcpStream, LdapAuthError> {
    let addrs = resolver
        .resolve_socket_addrs(host, port)
        .await
        .map_err(|_| LdapAuthError::ConnectionFailed)?;
    dial_admitted_addrs(addrs).await
}

async fn dial_admitted_addrs(addrs: Vec<std::net::SocketAddr>) -> Result<TcpStream, LdapAuthError> {
    for addr in addrs {
        if let Ok(stream) = TcpStream::connect(addr).await {
            return Ok(stream);
        }
    }
    Err(LdapAuthError::ConnectionFailed)
}

/// Phase 2: bind, then a best-effort unbind under the same deadline. The
/// outcome is decided by the bind result code; an unbind failure or a deadline
/// hit while unbinding does not change it.
async fn bind_phase<S: AsyncRead + AsyncWrite + Unpin>(
    mut session: BindSession<S>,
    budget: Duration,
    bind_dn: &str,
    password: &str,
) -> Result<(), LdapAuthError> {
    let deadline = tokio::time::Instant::now() + budget;
    let rc = tokio::time::timeout_at(deadline, session.simple_bind(bind_dn, password.as_bytes()))
        .await
        .map_err(|_| LdapAuthError::Timeout)?
        .map_err(|_| LdapAuthError::BindFailed)?
        .result_code;
    let _ = tokio::time::timeout_at(deadline, session.unbind()).await;
    match rc {
        0 => Ok(()),
        49 => Err(LdapAuthError::InvalidCredentials),
        _ => Err(LdapAuthError::BindFailed),
    }
}

/// Run phase 2 of `exchange` on an established session. Search mode returns
/// the entry's canonical identity to forward; bind mode returns `None` (the
/// client-typed username is forwarded).
async fn run_exchange<S: AsyncRead + AsyncWrite + Unpin>(
    session: BindSession<S>,
    budget: Duration,
    exchange: Exchange<'_>,
) -> Result<Option<String>, LdapAuthError> {
    match exchange {
        Exchange::Bind { dn, password } => bind_phase(session, budget, dn, password).await.map(|()| None),
        Exchange::Search {
            plan,
            service_password,
            username,
            password,
        } => search_phase(session, budget, plan, service_password, username, password)
            .await
            .map(Some),
    }
}

/// Search-mode phase 2: service bind, single-entry search, user bind, with
/// group requirements a service re-bind and a base-scope group check, then a
/// best-effort unbind, all under one deadline. Returns the canonical identity.
async fn search_phase<S: AsyncRead + AsyncWrite + Unpin>(
    mut session: BindSession<S>,
    budget: Duration,
    plan: &SearchPlan,
    service_password: &str,
    username: &str,
    password: &str,
) -> Result<String, LdapAuthError> {
    let deadline = tokio::time::Instant::now() + budget;
    let outcome = search_steps(&mut session, deadline, plan, service_password, username, password).await;
    let _ = tokio::time::timeout_at(deadline, session.unbind()).await;
    outcome
}

async fn search_steps<S: AsyncRead + AsyncWrite + Unpin>(
    session: &mut BindSession<S>,
    deadline: tokio::time::Instant,
    plan: &SearchPlan,
    service_password: &str,
    username: &str,
    password: &str,
) -> Result<String, LdapAuthError> {
    service_bind(session, deadline, plan, service_password).await?;
    let found = tokio::time::timeout_at(
        deadline,
        session.search_unique(
            &plan.base,
            Scope::Subtree,
            &plan.filter,
            username.as_bytes(),
            &plan.attribute,
            plan.time_limit_secs,
        ),
    )
    .await
    .map_err(|_| LdapAuthError::Timeout)?
    .map_err(|error| match error {
        // An entry the user bind cannot use is a per-user outcome.
        edgion_ldap::Error::EmptyDn => LdapAuthError::InvalidCredentials,
        _ => LdapAuthError::SearchFailed,
    })?;
    // No entry, several entries, or sizeLimitExceeded: the uniform credential path.
    let Some(found) = found else {
        return Err(LdapAuthError::InvalidCredentials);
    };
    // A DN this parser cannot read cannot be compared with `bindDn`: deny it
    // on the uniform path (fail closed, without revealing that it exists).
    // Never bind as the service account on behalf of a client either: that
    // would let clients log in as it or lock it out with wrong passwords.
    match edgion_ldap::dn::normalize(&found.dn) {
        Some(found_dn) if found_dn != plan.service_dn => {}
        _ => return Err(LdapAuthError::InvalidCredentials),
    }
    let Some(identity) = found.identity else {
        return Err(LdapAuthError::InvalidCredentials);
    };
    let rc = tokio::time::timeout_at(deadline, session.simple_bind(&found.dn, password.as_bytes()))
        .await
        .map_err(|_| LdapAuthError::Timeout)?
        .map_err(|_| LdapAuthError::BindFailed)?
        .result_code;
    match (rc, &plan.groups) {
        (0, None) => Ok(identity),
        // Group checks run only after the password is verified, so no group
        // outcome (403, a failing or slow group search) is observable without it.
        (0, Some(groups)) => {
            service_bind(session, deadline, plan, service_password).await?;
            let member = tokio::time::timeout_at(
                deadline,
                session.search_unique(&found.dn, Scope::Base, groups, b"", "1.1", plan.time_limit_secs),
            )
            .await
            .map_err(|_| LdapAuthError::Timeout)?
            .map_err(|_| LdapAuthError::SearchFailed)?;
            match member {
                Some(_) => Ok(identity),
                None => Err(LdapAuthError::NotInRequiredGroup),
            }
        }
        (49, _) => Err(LdapAuthError::InvalidCredentials),
        _ => Err(LdapAuthError::BindFailed),
    }
}

/// Bind as the service account: rc 49 is a rejected service password (the
/// generic-401 path), any other failure a search-mode dependency error.
async fn service_bind<S: AsyncRead + AsyncWrite + Unpin>(
    session: &mut BindSession<S>,
    deadline: tokio::time::Instant,
    plan: &SearchPlan,
    service_password: &str,
) -> Result<(), LdapAuthError> {
    let service = tokio::time::timeout_at(
        deadline,
        session.simple_bind(&plan.bind_dn, service_password.as_bytes()),
    )
    .await
    .map_err(|_| LdapAuthError::Timeout)?;
    match service.map(|result| result.result_code) {
        Ok(0) => Ok(()),
        Ok(49) => Err(LdapAuthError::ServiceBindRejected),
        _ => Err(LdapAuthError::SearchFailed),
    }
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

/// Build the LDAP rustls `ClientConfig` from the already-resolved outbound TLS settings.
///
/// - `verify=false` with no SAN policy → `AcceptAllVerifier` (operator's responsibility).
/// - `verify=false` with non-empty SAN policy → full webpki + `OutboundSanVerifier` (fail-secure:
///   SAN policy overrides skip-verify).
/// - `System` → webpki built-in roots (binary-embedded, not OS cert store).
/// - `Custom(pem)` → private CA bundle, REPLACES public roots entirely.
/// - Client identity (`client_identity`) → `with_client_auth_cert`; absent → `with_no_client_auth`.
/// - No ALPN protocols: LDAP does not negotiate ALPN.
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
    pub fn create(
        entry: &BoundPluginEntry<'_>,
        defaults: &GlobalOutboundTlsSnapshot,
    ) -> Option<Box<dyn RequestFilter>> {
        match Self::new(entry, defaults) {
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

    pub(crate) fn validate_tls_entry<'a>(
        entry: &'a BoundPluginEntry<'_>,
        defaults: &GlobalOutboundTlsSnapshot,
    ) -> Result<Option<ValidatedLocalTlsSelection<'a>>, String> {
        let error = || "LdapAuth TLS configuration error: invalid owning TLS reference".to_string();
        let EdgionPlugin::LdapAuth(config) = entry.config() else {
            return Err(error());
        };
        entry.owner_namespace().map_err(|_| error())?;
        // Plain LDAP does not consume local TLS material or inherited policy.
        if !config.ldaps && !config.start_tls {
            return Ok(None);
        }
        let selection = entry.outbound_tls_selection().map_err(|_| error())?;
        let proof = validate_local_tls_selection(selection).map_err(|_| error())?;
        if proof.active_field().is_none() {
            return Err(error());
        }
        validate_active_tls_inheritance(&proof, defaults).map_err(|_| error())?;
        Ok(Some(proof))
    }

    pub fn new(entry: &BoundPluginEntry<'_>, defaults: &GlobalOutboundTlsSnapshot) -> Result<Self, String> {
        let proof = Self::validate_tls_entry(entry, defaults)?;
        let EdgionPlugin::LdapAuth(config) = entry.config() else {
            return Err("LdapAuth TLS configuration error: invalid owning TLS reference".into());
        };
        // Cold validation in the constructor too: search mode must never send
        // the service password over a transport the runtime gate would reject.
        if let Some(error) = config.get_validation_error() {
            return Err(format!("LdapAuth configuration error: {error}"));
        }
        let timeout = Duration::from_millis(config.timeout);

        let mut verification_disabled = false;
        let tls_config = if let Some(proof) = proof {
            let resolved = resolve_validated_outbound_tls(&proof, true, defaults)
                .map_err(|e| format!("LdapAuth TLS configuration error: {e:?}"))?;
            verification_disabled = !resolved.verify
                && resolved
                    .san
                    .as_ref()
                    .and_then(|san| san.subject_alt_names.as_ref())
                    .is_none_or(|sans| sans.is_empty());

            // M4: when verify=false the resolver drops client_identity. Warn the
            // operator that their clientCertificateRef is not being used.
            if !resolved.verify && config.tls.client_certificate_ref.is_some() && resolved.client_identity.is_none() {
                tracing::warn!(
                    component = "ldap_auth",
                    "verify=false with clientCertificateRef configured: client certificate is not sent (M4)"
                );
            }

            Some(Arc::new(build_ldap_rustls_config(&resolved)?))
        } else {
            None
        };

        // Plain LDAP carries no TLS proof, so `tls_config` is `Some` exactly for
        // `ldaps` / `startTls` (the validator rejects both together). The
        // server name is parsed once; an unparsable host fails each request
        // with `ConnectionFailed` instead of failing plugin construction.
        let transport = match tls_config {
            None => LdapTransport::Plain,
            Some(tls_config) => {
                let connector = TlsConnector::from(tls_config);
                let server_name = ServerName::try_from(config.ldap_host.clone()).ok();
                if server_name.is_none() {
                    tracing::warn!(
                        component = "ldap_auth",
                        ldap_host = %config.ldap_host,
                        "ldapHost is not a valid TLS server name; every bind fails as ldap unreachable"
                    );
                }
                if config.ldaps {
                    LdapTransport::Ldaps { connector, server_name }
                } else {
                    LdapTransport::StartTls { connector, server_name }
                }
            }
        };

        let bind_dn_plan = match (&config.bind_dn, &config.bind_dn_template) {
            (Some(bind_dn), _) => {
                let filter = SearchFilter::compile(&config.effective_search_filter())
                    .map_err(|_| "LdapAuth configuration error: searchFilter is invalid".to_string())?;
                let service_dn = edgion_ldap::dn::normalize(bind_dn)
                    .ok_or_else(|| "LdapAuth configuration error: bindDn is not a valid DN".to_string())?;
                let groups = if config.has_group_requirements() {
                    fn trimmed(groups: &Option<Vec<String>>) -> Vec<&str> {
                        groups.iter().flatten().map(|group| group.trim()).collect()
                    }
                    let any_of = trimmed(&config.required_groups_any_of);
                    let all_of = trimmed(&config.required_groups_all_of);
                    let filter = SearchFilter::group_requirement(
                        config.group_attribute.trim(),
                        &any_of,
                        &all_of,
                        config.nested_groups,
                    )
                    .map_err(|_| "LdapAuth configuration error: required groups are invalid".to_string())?;
                    Some(filter)
                } else {
                    None
                };
                if verification_disabled {
                    tracing::warn!(
                        component = "ldap_auth",
                        "search mode with TLS verification disabled by the inherited default: the service-account password is exposed to an active man-in-the-middle"
                    );
                }
                BindDnPlan::Search(SearchPlan {
                    bind_dn: bind_dn.trim().to_string(),
                    service_dn,
                    password: config
                        .resolved_bind_password
                        .clone()
                        .filter(|password| !password.is_empty()),
                    base: config.search_base.clone().unwrap_or_else(|| config.base_dn.clone()),
                    filter,
                    attribute: config.attribute.trim().to_string(),
                    // Within `i32::MAX`, so the cast cannot truncate.
                    time_limit_secs: config.timeout.div_ceil(1000).min(i32::MAX as u64) as u32,
                    groups,
                })
            }
            (None, Some(template)) => BindDnPlan::Template(template.clone()),
            (None, None) => BindDnPlan::AttributeBase {
                attribute: config.attribute.clone(),
                base_dn: config.base_dn.clone(),
            },
        };

        let auth = AuthPolicy {
            header_type: config.header_type.clone(),
            hide_credentials: config.hide_credentials,
            auth_failure_delay_ms: config.auth_failure_delay_ms,
            anonymous: config.anonymous.clone(),
            auth_challenge: PreparedAuthChallenge::new(&config.header_type, &config.realm),
            credential_identifier_header: config.credential_identifier_header.clone(),
            anonymous_header: config.anonymous_header.clone(),
        };

        Ok(Self {
            ldap_host: config.ldap_host.clone(),
            ldap_port: config.ldap_port,
            transport,
            egress: ldap_egress_resolver(),
            timeout,
            bind_dn_plan,
            auth,
            #[cfg(test)]
            scripted_stream: parking_lot::Mutex::new(None),
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
        let leading_trimmed = raw_username.trim_start();
        let username_start = raw_username.len() - leading_trimmed.len();
        let username_end = username_start + leading_trimmed.trim_end().len();
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
        let escaped = edgion_ldap::dn::escape(username);

        match &self.bind_dn_plan {
            BindDnPlan::Template(template) => Ok(template.replace("{username}", escaped.as_ref())),
            BindDnPlan::AttributeBase { attribute, base_dn } => Ok(format!("{}={},{}", attribute, escaped, base_dn)),
            // Search mode finds the DN in the directory; it never builds one.
            BindDnPlan::Search(_) => Err("search mode builds no bind DN"),
        }
    }

    async fn ldap_bind(&self, exchange: Exchange<'_>) -> Result<Option<String>, LdapAuthError> {
        #[cfg(test)]
        if let Some(stream) = self.take_scripted_stream() {
            return self.bind_over(async { Ok(stream) }, exchange).await;
        }
        let dial = connect_admitted_ldap_tcp(&self.egress, &self.ldap_host, self.ldap_port);
        self.bind_over(dial, exchange).await
    }

    /// Run the bind exchange over the stream produced by `dial`.
    ///
    /// Two deadlines, each `self.timeout`: phase 1 covers dialing, StartTLS
    /// and the TLS handshake; phase 2 covers the exchange (`bind_phase` or
    /// `search_phase`) and the best-effort unbind.
    async fn bind_over<S, D>(&self, dial: D, exchange: Exchange<'_>) -> Result<Option<String>, LdapAuthError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send,
        D: Future<Output = Result<S, LdapAuthError>> + Send,
    {
        match &self.transport {
            LdapTransport::Plain => {
                let stream = tokio::time::timeout(self.timeout, dial)
                    .await
                    .map_err(|_| LdapAuthError::Timeout)??;
                run_exchange(BindSession::new(stream), self.timeout, exchange).await
            }
            LdapTransport::Ldaps { connector, server_name } => {
                let stream = tokio::time::timeout(self.timeout, async {
                    let name = server_name.clone().ok_or(LdapAuthError::ConnectionFailed)?;
                    let raw = dial.await?;
                    connector
                        .connect(name, raw)
                        .await
                        .map_err(|e| map_ldap_connect_error(&e))
                })
                .await
                .map_err(|_| LdapAuthError::Timeout)??;
                run_exchange(BindSession::new(stream), self.timeout, exchange).await
            }
            LdapTransport::StartTls { connector, server_name } => {
                let (stream, next_id) = tokio::time::timeout(self.timeout, async {
                    let name = server_name.clone().ok_or(LdapAuthError::ConnectionFailed)?;
                    let raw = dial.await?;
                    let (raw, next_id) = BindSession::new(raw)
                        .start_tls()
                        .await
                        .map_err(|_| LdapAuthError::ConnectionFailed)?;
                    let tls = connector
                        .connect(name, raw)
                        .await
                        .map_err(|e| map_ldap_connect_error(&e))?;
                    Ok::<_, LdapAuthError>((tls, next_id))
                })
                .await
                .map_err(|_| LdapAuthError::Timeout)??;
                run_exchange(BindSession::resume(stream, next_id), self.timeout, exchange).await
            }
        }
    }

    #[cfg(test)]
    fn install_scripted_stream(&self, stream: tokio::io::DuplexStream) {
        *self.scripted_stream.lock() = Some(stream);
    }

    #[cfg(test)]
    fn take_scripted_stream(&self) -> Option<tokio::io::DuplexStream> {
        self.scripted_stream.lock().take()
    }

    /// Write the authenticated identity, replacing any client-supplied value,
    /// and clear the anonymous marker. A failure must deny the request:
    /// otherwise a client-supplied identity header would reach the upstream.
    fn apply_authenticated_headers(
        &self,
        session: &mut dyn PluginSession,
        identity: &str,
    ) -> crate::plugins::runtime::PluginSessionResult<()> {
        set_sensitive_request_header(session, &self.auth.credential_identifier_header, identity)?;
        session.remove_request_header(&self.auth.anonymous_header)
    }

    fn apply_anonymous_headers(
        &self,
        session: &mut dyn PluginSession,
        anonymous_user: &str,
    ) -> crate::plugins::runtime::PluginSessionResult<()> {
        set_sensitive_request_header(session, &self.auth.credential_identifier_header, anonymous_user)?;
        set_sensitive_request_header(session, &self.auth.anonymous_header, "true")
    }

    fn hide_credentials_if_needed(&self, session: &mut dyn PluginSession) {
        if self.auth.hide_credentials {
            let _ = session.remove_request_header("authorization");
            let _ = session.remove_request_header("proxy-authorization");
        }
    }

    async fn unauthorized(&self, session: &mut dyn PluginSession) -> PluginRunningResult {
        let _ = send_gateway_auth_error_response(session, 401, &self.auth.auth_challenge, None, "Unauthorized").await;
        PluginRunningResult::ErrTerminateRequest
    }

    async fn service_unavailable(&self, session: &mut dyn PluginSession) -> PluginRunningResult {
        // Record BEFORE `send_gateway_plain_error_response`'s self-write
        // (`write_response_header`) so the queued dye header op is drained
        // by that write. This is the single choke point for every
        // LDAP-unreachable fail-close branch (connection failed, TLS SAN
        // mismatch, timeout, bind failure, search failure) — credential rejection
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
                    if self.apply_anonymous_headers(session, anon_user).is_err() {
                        plugin_log.push_err("identity header rejected");
                        return self.unauthorized(session).await;
                    }
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

        // Control characters (including TAB and NUL) never form a valid
        // identity: reject before any LDAP I/O so a directory that folds them
        // away cannot authenticate a name the identity header cannot carry.
        if credentials.username().chars().any(char::is_control) {
            drop(credentials);
            plugin_log.push("Deny bad username");
            apply_auth_failure_delay(self.auth.auth_failure_delay_ms).await;
            return self.unauthorized(session).await;
        }

        let bind_result = if let BindDnPlan::Search(plan) = &self.bind_dn_plan {
            if credentials.username().len() > MAX_SEARCH_USERNAME_LEN {
                drop(credentials);
                plugin_log.push("Deny bad username");
                apply_auth_failure_delay(self.auth.auth_failure_delay_ms).await;
                return self.unauthorized(session).await;
            }
            // An unresolved service secret answers with the generic 401
            // (design rule: misconfigured auth secrets never surface as 5xx)
            // and is counted so the outage is alertable.
            let Some(service_password) = plan.password.as_deref() else {
                drop(credentials);
                plugin_log.push_err("ldap service credential unavailable");
                record_ldap_auth_error("service_bind_fail");
                apply_auth_failure_delay(self.auth.auth_failure_delay_ms).await;
                return self.unauthorized(session).await;
            };
            self.ldap_bind(Exchange::Search {
                plan,
                service_password,
                username: credentials.username(),
                password: credentials.password(),
            })
            .await
        } else {
            let Ok(bind_dn) = self.build_bind_dn(credentials.username()) else {
                drop(credentials);
                plugin_log.push("Deny bad username");
                apply_auth_failure_delay(self.auth.auth_failure_delay_ms).await;
                return self.unauthorized(session).await;
            };
            let result = self
                .ldap_bind(Exchange::Bind {
                    dn: &bind_dn,
                    password: credentials.password(),
                })
                .await;
            drop(bind_dn);
            result
        };
        match bind_result {
            Ok(identity) => {
                // Search mode forwards the directory's canonical identity, not
                // the client-typed alias that matched the filter.
                let forwarded = identity.as_deref().unwrap_or(credentials.username());
                if self.apply_authenticated_headers(session, forwarded).is_err() {
                    drop(credentials);
                    plugin_log.push_err("identity header rejected");
                    return self.unauthorized(session).await;
                }
                self.hide_credentials_if_needed(session);
                plugin_log.push_fmt("OK user {}", la![sensitive(credentials.username())]);
                PluginRunningResult::GoodNext
            }
            Err(LdapAuthError::InvalidCredentials) => {
                drop(credentials);
                // Uniform credential-rejection tag, matching basic/key/hmac/jwt_auth.
                // LDAP rc 49 (invalidCredentials) is returned for both a nonexistent DN
                // and a wrong password, so this branch cannot — and must not appear to —
                // discriminate username existence.
                plugin_log.push("Deny bad credential");
                apply_auth_failure_delay(self.auth.auth_failure_delay_ms).await;
                self.unauthorized(session).await
            }
            Err(LdapAuthError::ConnectionFailed) => {
                drop(credentials);
                plugin_log.push_err("ldap unreachable");
                record_ldap_auth_error("unreachable");
                self.service_unavailable(session).await
            }
            Err(LdapAuthError::TlsSanMismatch(reason)) => {
                drop(credentials);
                session.record_error(edgion_resources::EdgionErrorCode::OutboundTlsSanMismatch);
                plugin_log.push_err("outbound tls san mismatch");
                plugin_log.push_fmt("san {}", la![reason.as_str()]);
                record_ldap_auth_error("tls_san_mismatch");
                record_outbound_tls_san_mismatch("ldap_auth");
                drop(reason);
                self.service_unavailable(session).await
            }
            Err(LdapAuthError::Timeout) => {
                drop(credentials);
                plugin_log.push_err("ldap timeout");
                record_ldap_auth_error("timeout");
                self.service_unavailable(session).await
            }
            Err(LdapAuthError::BindFailed) => {
                drop(credentials);
                plugin_log.push_err("ldap bind fail");
                record_ldap_auth_error("bind_fail");
                self.service_unavailable(session).await
            }
            Err(LdapAuthError::ServiceBindRejected) => {
                drop(credentials);
                // Generic 401 for a misconfigured auth secret; the metric
                // keeps the whole-plugin outage alertable.
                plugin_log.push_err("ldap service bind fail");
                record_ldap_auth_error("service_bind_fail");
                apply_auth_failure_delay(self.auth.auth_failure_delay_ms).await;
                self.unauthorized(session).await
            }
            Err(LdapAuthError::SearchFailed) => {
                drop(credentials);
                plugin_log.push_err("ldap search fail");
                record_ldap_auth_error("search_fail");
                self.service_unavailable(session).await
            }
            Err(LdapAuthError::NotInRequiredGroup) => {
                drop(credentials);
                // Authenticated but not authorized: reached only after the user
                // bind succeeded, so it never reveals anything without the password.
                plugin_log.push("Deny not in required group");
                apply_auth_failure_delay(self.auth.auth_failure_delay_ms).await;
                PluginRunningResult::ErrResponse {
                    status: 403,
                    body: Some(Bytes::from_static(b"403 Forbidden")),
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/ldap_auth/plugin_test.rs"]
mod tests;
