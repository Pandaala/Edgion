//! CRD `EtcdClientConfig` → `etcd_client::ConnectOptions` mapping.
//!
//! This module bridges the CRD configuration types (serde-driven, user-facing YAML)
//! with etcd-client's `ConnectOptions`. The mapping follows the Library-First principle:
//! we use etcd-client's API directly without inventing our own abstractions.

use std::time::Duration;

use anyhow::Result;
use etcd_client::{Certificate, ConnectOptions, Identity, TlsOptions};

use crate::link_sys_core::credential::CredentialState;
use crate::link_sys_core::outbound_tls::{self, EffectiveCa};
use edgion_resources::credentials::materials::BasicAuthMaterial;
use edgion_resources::resources::common::TlsConfig;
use edgion_resources::resources::link_sys::etcd::EtcdClientConfig;

// ============================================================================
// Safety ceilings for connection configuration
// ============================================================================

pub const MAX_DIAL_TIMEOUT_MS: u64 = 30_000;
pub const MAX_REQUEST_TIMEOUT_MS: u64 = 60_000;
const DEFAULT_DIAL_TIMEOUT_MS: u64 = 5_000;

// ============================================================================
// Config mapping (CRD → etcd-client ConnectOptions)
// ============================================================================

/// Map `EtcdClientConfig` (CRD) to `etcd_client::ConnectOptions`.
///
/// Returns `None` if no special options are needed (minimal config).
/// The caller passes the options to `Client::connect(endpoints, options)`.
///
/// `auth` is the [`CredentialState`] produced by
/// [`credential::basic_state`](crate::link_sys_core::credential::basic_state).
/// `Pending` / `Invalid` produce a fail-fast `bail!`.
pub fn build_connect_options(
    crd: &EtcdClientConfig,
    auth: &CredentialState<BasicAuthMaterial>,
) -> Result<Option<ConnectOptions>> {
    build_options(crd, auth, true)
}

/// Options for caller-injected IO. TLS is resolved by the canonical dialer.
pub fn admitted_connect_options(
    crd: &EtcdClientConfig,
    auth: &CredentialState<BasicAuthMaterial>,
) -> Result<ConnectOptions> {
    Ok(build_options(crd, auth, false)?.unwrap_or_else(default_connect_options))
}

fn build_options(
    crd: &EtcdClientConfig,
    auth: &CredentialState<BasicAuthMaterial>,
    include_tls: bool,
) -> Result<Option<ConnectOptions>> {
    let mut options = ConnectOptions::new();
    let mut has_options = false;

    if let Some(limit) = crd.max_call_send_size {
        options = options.with_max_send_message_size(limit);
        has_options = true;
    }
    if let Some(limit) = crd.max_call_recv_size {
        options = options.with_max_recv_message_size(limit);
        has_options = true;
    }
    if let Some(agent) = &crd.user_agent {
        options = options.with_user_agent(agent.clone());
        has_options = true;
    }
    if crd.auth.is_some() {
        let m = crate::link_sys_core::credential::require_basic_material(auth, "Etcd")?.ok_or_else(|| {
            anyhow::anyhow!("Etcd auth configured in CRD but credential state is NotRequired (internal bug)")
        })?;
        let user = m.username.as_deref().unwrap_or("");
        options.check_send_size_lower_bound(user.len().saturating_add(m.password.len()))?;
        options = options.with_user(user, m.password.as_str());
        has_options = true;
    }

    // TLS — parse CA / mTLS material from the CRD (inline + resolved Secret)
    // and feed it into etcd-client's `TlsOptions`.
    if let Some(tls) = &crd.tls {
        if tls.enabled && include_tls {
            let tls_options = build_tls_options(tls)?;
            options = options.with_tls(tls_options);
            has_options = true;
        }
    }

    // Dial (connection) timeout
    if let Some(timeout) = &crd.timeout {
        if let Some(dial_ms) = timeout.dial {
            let dial = dial_ms.min(MAX_DIAL_TIMEOUT_MS);
            options = options.with_connect_timeout(Duration::from_millis(dial));
            has_options = true;
        }
        // Per-request timeout
        if let Some(request_ms) = timeout.request {
            let request = request_ms.min(MAX_REQUEST_TIMEOUT_MS);
            options = options.with_timeout(Duration::from_millis(request));
            has_options = true;
        }
    }

    // HTTP/2 keep-alive
    if let Some(ka) = &crd.keep_alive {
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
    // own config-conversion layer: `EtcdConfig::to_client_config` in
    // edgion-controller fills `timeout.dial` and `keep_alive` before this
    // builder ever runs.
    Ok(if has_options { Some(options) } else { None })
}

/// Connect options for the DEDICATED leader-election client (spec §4.2/§3.2).
///
/// Identical to [`build_connect_options`] EXCEPT the channel-wide per-request
/// timeout is stripped. etcd's v3election `campaign()` is a unary RPC that
/// blocks server-side until this candidate wins; a channel-wide request timeout
/// would abort it every `request_timeout_ms`, turning a healthy standby into a
/// re-campaign hot loop and breaking active-passive HA. h2 keepalive is retained
/// (from the shared builder) so a blackholed campaign connection still dies.
pub fn election_connect_options(
    crd: &EtcdClientConfig,
    auth: &CredentialState<BasicAuthMaterial>,
) -> Result<ConnectOptions> {
    let mut crd = crd.clone();
    if let Some(t) = crd.timeout.as_mut() {
        t.request = None;
    }
    Ok(build_connect_options(&crd, auth)?.unwrap_or_else(default_connect_options))
}

/// Build TLS options from CRD config.
///
/// Uses the shared [`outbound_tls::resolve_outbound_tls`] resolver with
/// `provider_can_disable: false` — etcd-client's TLS stack always verifies
/// certificates and has no skip-verify knob, so the resolver will return
/// [`EdgionErrorCode::OutboundTlsVerifyUnsupported`] (mapped to an error here)
/// if `verify=false` is configured, preventing a silent ignore of the
/// operator's intent.
fn build_tls_options(tls: &TlsConfig) -> Result<TlsOptions> {
    let resolved = outbound_tls::resolve_outbound_tls(
        &tls.outbound,
        tls.resolved_ca_certificates.as_deref(),
        tls.resolved_client_certificate.as_ref(),
        false, // etcd-client cannot disable TLS verification
        "Etcd",
    )
    .map_err(|e| {
        anyhow::anyhow!(
            "Etcd TLS configuration error: {e:?}. \
             etcd-client does not support skip-verify; \
             use a proper CA certificate or connect without TLS."
        )
    })?;

    resolved.reject_unsupported_identity_policy("Etcd")?;

    let mut tls_options = TlsOptions::new();

    if let EffectiveCa::Custom(bundles) = &resolved.ca {
        // etcd-client accepts one Certificate per `ca_certificate()` call;
        // iterate over all resolved PEM bundles and add each in turn.
        for bundle in bundles {
            tls_options = tls_options.ca_certificate(Certificate::from_pem(bundle.as_slice()));
        }
    }

    if let Some((cert_pem, key_pem)) = &resolved.client_identity {
        tls_options = tls_options.identity(Identity::from_pem(cert_pem, key_pem.as_slice()));
    }

    Ok(tls_options)
}

/// Get default connect options with a reasonable dial timeout.
/// Used when no specific options are configured but we still want a timeout.
pub fn default_connect_options() -> ConnectOptions {
    // Shared fallback for EVERY fully-unconfigured EtcdLinkClient (LinkSys
    // included) — dial timeout only, no h2 keepalive, exactly the pre-election
    // behavior. The conf_center client never reaches this fallback: its
    // config-conversion layer (`EtcdConfig::to_client_config`) always fills
    // `timeout.dial` and `keep_alive`, which is where the leader-election
    // blackhole-detection defaults (spec §4.2 fact 2) live.
    ConnectOptions::new().with_connect_timeout(Duration::from_millis(DEFAULT_DIAL_TIMEOUT_MS))
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
    crd.timeout
        .as_ref()
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
