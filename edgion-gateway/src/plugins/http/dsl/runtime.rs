//! DslRuntime — per-request execution context for external capabilities

use std::collections::HashMap;
use std::sync::OnceLock;

/// Per-execution limits for external calls, constructed at request time from DslConfig.
pub struct DslRuntime {
    /// Maximum wall-clock timeout for a single http.call (ms)
    pub max_timeout_ms: u64,
    /// Maximum response body size from http.call (bytes)
    pub max_response_body_bytes: usize,
    /// When true, http.call rejects URLs whose host resolves to a private/loopback IP.
    /// Covers direct IP literals; hostname-based SSRF requires network-level egress controls.
    pub block_private_ips: bool,
    /// Maps script alias → "namespace/name" key for get_redis_client() lookups.
    /// Built once from linkSysRefs at plugin load time.
    /// Client is resolved per-call (ArcSwap read) — matches rate_limit_redis pattern.
    pub redis_links: HashMap<String, String>,
}

impl Default for DslRuntime {
    fn default() -> Self {
        Self {
            max_timeout_ms: 5_000,
            max_response_body_bytes: 1024 * 1024, // 1 MB
            block_private_ips: false,
            redis_links: HashMap::new(),
        }
    }
}

/// Maximum redirects followed by the shared HTTP client. Applied globally at client init.
const DSL_MAX_REDIRECTS: usize = 10;

static DSL_HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
static DSL_HTTP_CLIENT_GUARDED: OnceLock<reqwest::Client> = OnceLock::new();

/// Returns the process-global shared reqwest client for DSL http.call.
///
/// Selection is keyed by the per-request `block_private_ips` flag:
/// - false → permissive client (`Policy::limited(10)`)
/// - true  → guarded client whose `Policy::custom` re-applies
///   `is_ssrf_blocked_ip_literal` to every redirect hop's host.
///   Same 10-hop cap as the permissive client.
///
/// Both clients are process-global so connection pools are reused across calls.
pub(crate) fn dsl_http_client(block_private_ips: bool) -> &'static reqwest::Client {
    if block_private_ips {
        DSL_HTTP_CLIENT_GUARDED.get_or_init(|| {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::custom(|attempt| {
                    if attempt.previous().len() >= DSL_MAX_REDIRECTS {
                        return attempt.stop();
                    }
                    let raw = attempt.url().host_str().unwrap_or("");
                    // url crate serializes IPv6 hosts with brackets (e.g. "[::1]") — strip them
                    let host = raw.trim_matches(|c: char| c == '[' || c == ']');
                    if super::lang::builtins::is_ssrf_blocked_ip_literal(host) {
                        return attempt.error("redirect to private/loopback IP blocked");
                    }
                    attempt.follow()
                }))
                .pool_max_idle_per_host(4)
                // Disable reqwest's default HTTP_PROXY/HTTPS_PROXY/NO_PROXY env pickup, matching
                // the project's peer internal clients (all_endpoint_status, health probes). DSL
                // outbound must not be silently rerouted through a Pod-injected corporate proxy.
                .no_proxy()
                .build()
                .expect("hardcoded builder options (no custom resolver, proxy disabled, no explicit cert store path) always build successfully")
        })
    } else {
        DSL_HTTP_CLIENT.get_or_init(|| {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::limited(DSL_MAX_REDIRECTS))
                .pool_max_idle_per_host(4)
                // See guarded-client branch above for rationale.
                .no_proxy()
                .build()
                .expect("hardcoded builder options (no custom resolver, proxy disabled, no explicit cert store path) always build successfully")
        })
    }
}
