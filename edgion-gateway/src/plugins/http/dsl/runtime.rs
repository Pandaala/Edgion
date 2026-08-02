//! DslRuntime — per-request execution context for external capabilities

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use edgion_common::dns::ResolverSpec;

use crate::link_sys::http_client::{build_outbound_client, OutboundClientConfig, OutboundRedirect};
use crate::plugins::http::common::outbound::egress::{egress_dns_cache_ttl, EgressPolicy, EgressResolver};

/// Per-execution limits for external calls, constructed at request time from DslConfig.
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
    /// Cumulative request-body scan allowance, measured in physical-cap passes.
    pub max_body_scan_passes: u8,
}

impl Default for DslRuntime {
    fn default() -> Self {
        Self {
            max_timeout_ms: 5_000,
            max_response_body_bytes: 1024 * 1024, // 1 MB
            max_request_body_bytes: 1024 * 1024,  // 1 MiB
            block_private_ips: false,
            redis_links: HashMap::new(),
            max_body_scan_passes: 4,
        }
    }
}

/// Maximum redirects followed by the shared HTTP client. Applied globally at client init.
const DSL_MAX_REDIRECTS: usize = 10;

static DSL_HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
static DSL_HTTP_CLIENT_GUARDED: OnceLock<reqwest::Client> = OnceLock::new();
static DSL_HTTP_CLIENT_NO_REDIRECT: OnceLock<reqwest::Client> = OnceLock::new();
static DSL_HTTP_CLIENT_GUARDED_NO_REDIRECT: OnceLock<reqwest::Client> = OnceLock::new();

fn dsl_egress_resolver() -> Arc<dyn reqwest::dns::Resolve> {
    Arc::new(EgressResolver::new(
        ResolverSpec::System,
        EgressPolicy { block_private: true },
        egress_dns_cache_ttl(),
    ))
}

fn build_dsl_http_client(block_private_ips: bool, follow_redirects: bool) -> reqwest::Client {
    let redirect = match (block_private_ips, follow_redirects) {
        (_, false) => OutboundRedirect::Deny,
        (false, true) => OutboundRedirect::Limit(DSL_MAX_REDIRECTS),
        (true, true) => {
            let policy = reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= DSL_MAX_REDIRECTS {
                    return attempt.stop();
                }
                let raw = attempt.url().host_str().unwrap_or("");
                // url crate serializes IPv6 hosts with brackets (e.g. "[::1]").
                let host = raw.trim_matches(|c: char| c == '[' || c == ']');
                if super::lang::builtins::is_ssrf_blocked_ip_literal(host) {
                    return attempt.error("redirect to private/loopback IP blocked");
                }
                attempt.follow()
            });
            OutboundRedirect::Custom(policy)
        }
    };

    build_outbound_client(OutboundClientConfig {
        pool_max_idle_per_host: Some(4),
        // Do not allow Pod-injected proxy environment variables to reroute DSL egress.
        disable_proxy: true,
        redirect,
        dns_resolver: block_private_ips.then(dsl_egress_resolver),
        ..Default::default()
    })
    .expect("DSL HTTP client construction uses only fixed pool, redirect, proxy, and system-resolver options")
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
    match (block_private_ips, follow_redirects) {
        (false, true) => DSL_HTTP_CLIENT.get_or_init(|| build_dsl_http_client(false, true)),
        (true, true) => DSL_HTTP_CLIENT_GUARDED.get_or_init(|| build_dsl_http_client(true, true)),
        (false, false) => DSL_HTTP_CLIENT_NO_REDIRECT.get_or_init(|| build_dsl_http_client(false, false)),
        (true, false) => DSL_HTTP_CLIENT_GUARDED_NO_REDIRECT.get_or_init(|| build_dsl_http_client(true, false)),
    }
}
