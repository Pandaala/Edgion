//! DslRuntime — per-request execution context for external capabilities

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
}

impl Default for DslRuntime {
    fn default() -> Self {
        Self {
            max_timeout_ms: 5_000,
            max_response_body_bytes: 1024 * 1024, // 1 MB
            block_private_ips: false,
        }
    }
}

/// Maximum redirects followed by the shared HTTP client. Applied globally at client init.
const DSL_MAX_REDIRECTS: usize = 10;

static DSL_HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// Returns the process-global shared reqwest client for DSL http.call.
pub(crate) fn dsl_http_client() -> &'static reqwest::Client {
    DSL_HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(DSL_MAX_REDIRECTS))
            .pool_max_idle_per_host(4)
            .build()
            .expect("failed to build DSL HTTP client")
    })
}
