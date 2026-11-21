//! DslRuntime — immutable per-plugin policy for external capabilities

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use crate::plugins::common::redis_rate_limiter::RedisRateLimiter;
use edgion_common::dns::ResolverSpec;
use edgion_resources::resources::edgion_plugins::plugin_configs::dsl::{
    DslConfig, DSL_DEFAULT_HTTP_MAX_REQUEST_BODY_BYTES, DSL_DEFAULT_HTTP_MAX_RESPONSE_BODY_BYTES,
    DSL_DEFAULT_HTTP_MAX_TIMEOUT_MS, DSL_DEFAULT_MAX_BODY_SCAN_PASSES,
};
use edgion_resources::resources::edgion_plugins::RateLimitAlgorithm;
use std::time::Duration;

use crate::link_sys::http_client::{build_outbound_client, OutboundClientConfig, OutboundRedirect};
use crate::plugins::http::common::outbound::egress::{
    egress_dns_cache_ttl, EgressPolicy, EgressResolver, ResolverAddressPolicy,
};

/// Per-plugin external-call policy, built once by `resolve_parts` for each config generation.
///
/// Request execution borrows this immutable snapshot from the shared `DslPlugin`.
pub struct DslRuntime {
    /// Maximum wall-clock timeout for a single http.call (ms)
    pub max_timeout_ms: u64,
    /// Maximum response body size from http.call (bytes)
    pub max_response_body_bytes: usize,
    /// Maximum opaque request Body size sent by one http.call (bytes).
    pub max_request_body_bytes: usize,
    /// When true, http.call rejects private/loopback literals and resolved addresses.
    pub block_private_ips: bool,
    /// Maps script alias → "namespace/name" key for get_redis_client() lookups.
    /// Built once from linkSysRefs at plugin load time.
    /// Client is resolved per-call (ArcSwap read) — matches rate_limit_redis pattern.
    pub redis_links: HashMap<String, String>,
    /// Immutable policies and shared Lua executor, built at plugin load.
    pub rate_limits: DslRateLimits,
    /// Cumulative request-body scan allowance, measured in physical-cap passes.
    pub max_body_scan_passes: u8,
}

impl Default for DslRuntime {
    fn default() -> Self {
        Self {
            max_timeout_ms: DSL_DEFAULT_HTTP_MAX_TIMEOUT_MS,
            max_response_body_bytes: DSL_DEFAULT_HTTP_MAX_RESPONSE_BODY_BYTES,
            max_request_body_bytes: DSL_DEFAULT_HTTP_MAX_REQUEST_BODY_BYTES,
            block_private_ips: false,
            redis_links: HashMap::new(),
            rate_limits: DslRateLimits::default(),
            max_body_scan_passes: DSL_DEFAULT_MAX_BODY_SCAN_PASSES,
        }
    }
}

/// Config-owned policy table; there is no process-global mutable DSL counter cache.
#[derive(Default)]
pub struct DslRateLimits {
    pub(super) policies: HashMap<String, ResolvedRateLimitPolicy>,
    pub(super) limiter: RedisRateLimiter,
}

pub(super) struct ResolvedRateLimitPolicy {
    pub link: String,
    pub prefix: String,
    pub algorithm: RateLimitAlgorithm,
    pub rate: u64,
    pub window_ms: i64,
    pub timeout: Duration,
}

impl DslRateLimits {
    /// Validate before constructing immutable policy snapshots, including callers
    /// that do not enter through the normal plugin factory.
    pub fn from_config(config: &DslConfig) -> Result<Self, String> {
        config.validate()?;
        let mut policies = HashMap::with_capacity(config.rate_limit_policies.len());
        for policy in &config.rate_limit_policies {
            let window_ms = policy.validate()?;
            // Length-delimit operator identity; numeric parameters select a fresh
            // generation on reload. No raw client key is truncated or hashed.
            let prefix = format!(
                "edgion:dsl:rl:{}:{}:{}:{}:{}:{}:",
                policy.namespace.len(),
                policy.namespace,
                policy.name.len(),
                policy.name,
                policy.rate,
                window_ms
            );
            policies.insert(
                policy.name.clone(),
                ResolvedRateLimitPolicy {
                    link: policy.link.clone(),
                    prefix,
                    algorithm: policy.algorithm.clone(),
                    rate: policy.rate,
                    window_ms,
                    timeout: Duration::from_millis(policy.timeout_ms),
                },
            );
        }
        Ok(Self {
            policies,
            limiter: RedisRateLimiter::new(),
        })
    }
}

/// Maximum redirects followed by the shared HTTP client. Applied globally at client init.
const DSL_MAX_REDIRECTS: usize = 10;

static DSL_HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
static DSL_HTTP_CLIENT_GUARDED: OnceLock<reqwest::Client> = OnceLock::new();
static DSL_HTTP_CLIENT_NO_REDIRECT: OnceLock<reqwest::Client> = OnceLock::new();
static DSL_HTTP_CLIENT_GUARDED_NO_REDIRECT: OnceLock<reqwest::Client> = OnceLock::new();

/// DSL destinations come from scripts fed by request data and from upstream
/// redirects, so they use the request-influenced answer cache.
fn dsl_egress_resolver(block_private_ips: bool) -> Arc<dyn reqwest::dns::Resolve> {
    Arc::new(EgressResolver::new_request_influenced(
        ResolverSpec::System,
        if block_private_ips {
            ResolverAddressPolicy::Standard(EgressPolicy { block_private: true })
        } else {
            ResolverAddressPolicy::ConfiguredCidrsOnly
        },
        egress_dns_cache_ttl(),
    ))
}

fn dsl_http_client_config(block_private_ips: bool, follow_redirects: bool) -> OutboundClientConfig {
    let guarded = block_private_ips || edgion_common::egress::has_blocked_cidrs();
    let redirect = match (guarded, follow_redirects) {
        (_, false) => OutboundRedirect::Deny,
        (false, true) => OutboundRedirect::Limit(DSL_MAX_REDIRECTS),
        (true, true) => {
            let policy = reqwest::redirect::Policy::custom(move |attempt| {
                if block_private_ips && attempt.previous().len() >= DSL_MAX_REDIRECTS {
                    return attempt.stop();
                }
                let raw = attempt.url().host_str().unwrap_or("");
                // url crate serializes IPv6 hosts with brackets (e.g. "[::1]").
                let host = raw.trim_matches(|c: char| c == '[' || c == ']');
                if super::lang::builtins::is_ssrf_blocked_ip_literal(host, block_private_ips) {
                    return attempt.error("redirect destination blocked by egress policy");
                }
                reqwest::redirect::Policy::limited(DSL_MAX_REDIRECTS).redirect(attempt)
            });
            OutboundRedirect::Custom(policy)
        }
    };

    OutboundClientConfig {
        pool_max_idle_per_host: Some(4),
        // Do not allow Pod-injected proxy environment variables to reroute DSL egress.
        disable_proxy: true,
        redirect,
        dns_resolver: guarded.then(|| dsl_egress_resolver(block_private_ips)),
        ..Default::default()
    }
}

/// Returns the process-global shared reqwest client for DSL http.call.
///
/// Selection is keyed by the per-plugin private-address policy and whether the
/// payload is replayable. Opaque Body requests never follow redirects; String
/// and Nil requests retain the existing ten-hop policy. Guarded clients also
/// classify resolved addresses at dial time.
///
/// All four clients are process-global so connection pools are reused.
pub(crate) fn dsl_http_client(block_private_ips: bool, follow_redirects: bool) -> &'static reqwest::Client {
    let client = match (block_private_ips, follow_redirects) {
        (false, true) => &DSL_HTTP_CLIENT,
        (true, true) => &DSL_HTTP_CLIENT_GUARDED,
        (false, false) => &DSL_HTTP_CLIENT_NO_REDIRECT,
        (true, false) => &DSL_HTTP_CLIENT_GUARDED_NO_REDIRECT,
    };
    client.get_or_init(|| {
        build_outbound_client(dsl_http_client_config(block_private_ips, follow_redirects))
            .expect("DSL HTTP client construction uses only fixed pool, redirect, proxy, and system-resolver options")
    })
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/dsl/runtime_test.rs"]
mod tests;
