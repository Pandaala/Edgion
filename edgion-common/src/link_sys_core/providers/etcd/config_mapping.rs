//! Actual LinkSys and trusted bootstrap inputs to retained Etcd SDK options.
//!
//! Both entries use the shared borrowed TLS core and one scalar/options mapper.
//! SDK options are prepared once for ordinary and dedicated election connections.

use std::time::Duration;

use anyhow::Result;
use etcd_client::{Certificate, ConnectOptions, Identity, TlsOptions};

use crate::link_sys_core::credential::CredentialState;
use crate::link_sys_core::outbound_tls::{self, EffectiveCa, GlobalOutboundTlsSnapshot, PreparedLinkSysTls};
use edgion_resources::credentials::materials::BasicAuthMaterial;
use edgion_resources::resources::link_sys::etcd::{
    validate_etcd_connection_fields, EtcdClientConfig, EtcdKeepAlive, EtcdTimeout,
};
use edgion_resources::resources::link_sys::SystemConfig;

// ============================================================================
// Safety ceilings for connection configuration
// ============================================================================

pub const MAX_DIAL_TIMEOUT_MS: u64 = 30_000;
pub const MAX_REQUEST_TIMEOUT_MS: u64 = 60_000;
const DEFAULT_DIAL_TIMEOUT_MS: u64 = 5_000;

// ============================================================================
// Config mapping (CRD → etcd-client ConnectOptions)
// ============================================================================

pub(super) struct PreparedOptions {
    pub(super) ordinary: ConnectOptions,
    pub(super) election: ConnectOptions,
}

fn validate_fields(
    endpoints: &[String],
    timeout: Option<&EtcdTimeout>,
    keep_alive: Option<&EtcdKeepAlive>,
) -> Result<()> {
    let errors = validate_etcd_connection_fields(endpoints, timeout, keep_alive);
    anyhow::ensure!(errors.is_empty(), "Invalid Etcd configuration: {}", errors.join("; "));
    Ok(())
}

fn auth_material(declared: bool, auth: &CredentialState<BasicAuthMaterial>) -> Result<Option<&BasicAuthMaterial>> {
    if !declared {
        return Ok(None);
    }
    let material = crate::link_sys_core::credential::require_basic_material(auth, "Etcd")?.ok_or_else(|| {
        anyhow::anyhow!("Etcd auth configured in CRD but credential state is NotRequired (internal bug)")
    })?;
    Ok(Some(material))
}

pub(super) fn build_owned_options(input: &PreparedLinkSysTls) -> Result<PreparedOptions> {
    let SystemConfig::Etcd(config) = &input.owner().spec.config else {
        anyhow::bail!("Etcd construction requires an actual Etcd LinkSys");
    };
    validate_fields(&config.endpoints, config.timeout.as_ref(), config.keep_alive.as_ref())?;
    let auth = crate::link_sys_core::credential::basic_state(input.owner());
    anyhow::ensure!(
        auth.is_buildable(),
        "Etcd construction called with non-buildable credential state: {}",
        auth.kind()
    );
    let tls = input.with_tls_selection(|proof, defaults| {
        if proof.active_field().is_none() {
            // Inactive TLS uses System trust without global inheritance.
            return Ok(default_tls_options(&config.endpoints));
        }
        let resolved = outbound_tls::resolve_validated_outbound_tls_audited(proof, false, "Etcd", defaults)
            .map_err(tls_configuration_error)?;
        tls_options_from_resolved(resolved).map(Some)
    })?;
    let base = build_base_options(
        config.timeout.as_ref(),
        config.keep_alive.as_ref(),
        auth_material(config.auth.is_some(), &auth)?,
        tls,
    );
    Ok(finish_options(base, config.timeout.as_ref()))
}

pub(super) fn build_bootstrap_options(
    config: &super::client::EtcdBootstrapConfig,
    defaults: Option<&GlobalOutboundTlsSnapshot>,
) -> Result<PreparedOptions> {
    validate_fields(&config.endpoints, config.timeout.as_ref(), config.keep_alive.as_ref())?;
    let tls = match config.tls.as_ref() {
        None => default_tls_options(&config.endpoints),
        Some(material) => {
            let defaults = defaults.ok_or_else(|| anyhow::anyhow!("Etcd bootstrap TLS requires captured defaults"))?;
            let resolved = outbound_tls::resolve_etcd_bootstrap_tls_audited(material, defaults)
                .map_err(tls_configuration_error)?;
            Some(tls_options_from_resolved(resolved)?)
        }
    };
    let base = build_base_options(
        config.timeout.as_ref(),
        config.keep_alive.as_ref(),
        config.auth.as_ref(),
        tls,
    );
    Ok(finish_options(base, config.timeout.as_ref()))
}

/// One scalar/options mapper for actual resource and trusted bootstrap inputs.
/// The base deliberately excludes unary request timeout for election reuse.
fn build_base_options(
    timeout: Option<&EtcdTimeout>,
    keep_alive: Option<&EtcdKeepAlive>,
    auth: Option<&BasicAuthMaterial>,
    tls: Option<TlsOptions>,
) -> Option<ConnectOptions> {
    let mut options = ConnectOptions::new().with_auto_token_refresh(false);
    let mut has_options = false;

    if let Some(m) = auth {
        let user = m.username.as_deref().unwrap_or("");
        options = options.with_user(user, m.password.as_str());
        has_options = true;
    }

    if let Some(tls) = tls {
        options = options.with_tls(tls);
        has_options = true;
    }

    // Dial (connection) timeout
    if let Some(timeout) = timeout {
        if let Some(dial_ms) = timeout.dial {
            options = options.with_connect_timeout(clamped_dial_timeout(dial_ms));
            has_options = true;
        }
    }

    // HTTP/2 keep-alive
    if let Some(ka) = keep_alive {
        if let (Some(time), Some(timeout)) = (ka.time, ka.timeout) {
            options = options.with_keep_alive(Duration::from_secs(time), Duration::from_secs(timeout));
            has_options = true;

            if ka.permit_without_stream == Some(true) {
                options = options.with_keep_alive_while_idle(true);
            }
        }
    }

    // NOTE deliberately NO unconditional defaults here: this builder is shared
    // by EVERY EtcdLinkClient — the gateway's operator-configured LinkSys etcd
    // clients included — and injecting h2-keepalive/dial defaults here would
    // silently change their connection behavior in every mode (a §10.5
    // zero-K8s-impact violation). The conf_center client, which DOES need
    // always-on h2 keepalive (spec §4.2 fact 2: h2 PING is the only thing that
    // kills a blackholed established stream), gets its defaults injected at its
    // own config-conversion layer: `EtcdConfig::to_bootstrap_config` in
    // edgion-controller fills `timeout.dial` and `keep_alive` before this
    // builder ever runs.
    if has_options {
        Some(options)
    } else {
        None
    }
}

#[allow(
    clippy::unwrap_or_default,
    reason = "etcd-client ConnectOptions::new enables idle keepalive; derived Default does not"
)]
fn finish_options(base: Option<ConnectOptions>, timeout: Option<&EtcdTimeout>) -> PreparedOptions {
    let (ordinary_timeout, election_timeout) =
        connection_timeout_overrides(base.is_some(), timeout.and_then(|timeout| timeout.request));
    let election = apply_timeout_overrides(base.clone().unwrap_or_else(ConnectOptions::new), election_timeout);
    let ordinary = apply_timeout_overrides(base.unwrap_or_else(ConnectOptions::new), ordinary_timeout);
    PreparedOptions { ordinary, election }
}

#[derive(Debug, PartialEq, Eq)]
struct ConnectionTimeoutOverrides {
    dial: Option<Duration>,
    request: Option<Duration>,
}

/// Ordinary request-only options preserve an absent dial timeout. Election
/// always omits unary deadlines and uses the bounded fallback for an empty base.
fn connection_timeout_overrides(
    base_present: bool,
    request: Option<u64>,
) -> (ConnectionTimeoutOverrides, ConnectionTimeoutOverrides) {
    (
        ConnectionTimeoutOverrides {
            dial: (!base_present && request.is_none()).then(|| clamped_dial_timeout(DEFAULT_DIAL_TIMEOUT_MS)),
            request: request.map(|ms| Duration::from_millis(ms.min(MAX_REQUEST_TIMEOUT_MS))),
        },
        ConnectionTimeoutOverrides {
            dial: (!base_present).then(|| clamped_dial_timeout(DEFAULT_DIAL_TIMEOUT_MS)),
            request: None,
        },
    )
}

fn clamped_dial_timeout(ms: u64) -> Duration {
    Duration::from_millis(ms.min(MAX_DIAL_TIMEOUT_MS))
}

fn apply_timeout_overrides(mut options: ConnectOptions, timeout: ConnectionTimeoutOverrides) -> ConnectOptions {
    if let Some(dial) = timeout.dial {
        options = options.with_connect_timeout(dial);
    }
    if let Some(request) = timeout.request {
        options = options.with_timeout(request);
    }
    options
}

/// Preserve the existing capability error text for keyed construction failures.
/// The canonical resolver rejects skip-verify before an SDK connection is made.
fn tls_configuration_error(error: edgion_resources::EdgionErrorCode) -> anyhow::Error {
    anyhow::anyhow!(
        "Etcd TLS configuration error: {error:?}. \
             etcd-client does not support skip-verify; \
             use a proper CA certificate or connect without TLS."
    )
}

/// etcd-client gives `https://` endpoints an empty tonic TLS config when no
/// options are set, which trusts no root. Without a TLS block, an all-HTTPS
/// endpoint set therefore gets `System` trust, the meaning of an omitted CA
/// everywhere else. Other endpoint sets keep no options: etcd-client rejects
/// TLS options for `http://` endpoints and applies them to scheme-less ones.
fn uses_default_system_tls(endpoints: &[String]) -> bool {
    !endpoints.is_empty() && endpoints.iter().all(|endpoint| endpoint.starts_with("https://"))
}

fn default_tls_options(endpoints: &[String]) -> Option<TlsOptions> {
    uses_default_system_tls(endpoints).then(|| {
        ca_pem_bundles(&EffectiveCa::System)
            .into_iter()
            .fold(TlsOptions::new(), |options, bundle| {
                options.ca_certificate(Certificate::from_pem(bundle))
            })
    })
}

/// The PEM bundles handed to etcd-client, one per `ca_certificate()` call.
/// tonic loads no roots implicitly, so System trust must be passed explicitly.
fn ca_pem_bundles(ca: &EffectiveCa) -> Vec<&[u8]> {
    match ca {
        EffectiveCa::Custom(bundles) => bundles.iter().map(Vec::as_slice).collect(),
        EffectiveCa::System => vec![crate::system_trust::system_trust().pem()],
    }
}

fn tls_options_from_resolved(resolved: outbound_tls::ResolvedOutboundTls) -> Result<TlsOptions> {
    resolved.reject_unsupported_identity_policy("Etcd")?;

    let mut tls_options = TlsOptions::new();
    for bundle in ca_pem_bundles(&resolved.ca) {
        tls_options = tls_options.ca_certificate(Certificate::from_pem(bundle));
    }

    if let Some((cert_pem, key_pem)) = &resolved.client_identity {
        tls_options = tls_options.identity(Identity::from_pem(cert_pem, key_pem.as_slice()));
    }

    Ok(tls_options)
}

/// Default ceiling for live health-probe `status()` calls when the CRD does not
/// set an explicit per-request timeout. 5s matches the Redis provider's
/// DEFAULT_COMMAND_TIMEOUT_MS, keeping cross-provider health semantics aligned.
const DEFAULT_HEALTH_PROBE_TIMEOUT_MS: u64 = 5_000;

/// Resolve the per-call timeout for a health-probe `status()`: prefer the
/// operator-configured request timeout, else the 5s default. Clamped to
/// `MAX_REQUEST_TIMEOUT_MS` so a misconfiguration cannot disable the ceiling.
///
/// etcd-client already honors a configured request timeout internally; this
/// wrap is a hard backstop whose real value is protecting the *unconfigured*
/// case (where a stuck etcd would otherwise hang the call forever).
pub fn health_probe_timeout(crd: &EtcdClientConfig) -> Duration {
    probe_timeout_for(crd.timeout.as_ref())
}

pub(super) fn probe_timeout_for(timeout: Option<&EtcdTimeout>) -> Duration {
    timeout
        .and_then(|t| t.request)
        .map(|ms| Duration::from_millis(ms.min(MAX_REQUEST_TIMEOUT_MS)))
        .unwrap_or_else(|| Duration::from_millis(DEFAULT_HEALTH_PROBE_TIMEOUT_MS))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[path = "../../../../tests/unit/link_sys_core/providers/etcd/config_mapping_test.rs"]
mod tests;
