#[cfg(feature = "body-buffering")]
extern crate pingora_core_edgion as pingora_core;
#[cfg(feature = "body-buffering")]
extern crate pingora_http_edgion as pingora_http;
#[cfg(feature = "body-buffering")]
extern crate pingora_ketama_edgion as pingora_ketama;
#[cfg(feature = "body-buffering")]
extern crate pingora_limits_edgion as pingora_limits;
#[cfg(feature = "body-buffering")]
extern crate pingora_load_balancing_edgion as pingora_load_balancing;
#[cfg(feature = "body-buffering")]
extern crate pingora_proxy_edgion as pingora_proxy;

#[cfg(all(feature = "upstream-pingora", not(feature = "body-buffering")))]
extern crate pingora_core_upstream as pingora_core;
#[cfg(all(feature = "upstream-pingora", not(feature = "body-buffering")))]
extern crate pingora_http_upstream as pingora_http;
#[cfg(all(feature = "upstream-pingora", not(feature = "body-buffering")))]
extern crate pingora_ketama_upstream as pingora_ketama;
#[cfg(all(feature = "upstream-pingora", not(feature = "body-buffering")))]
extern crate pingora_limits_upstream as pingora_limits;
#[cfg(all(feature = "upstream-pingora", not(feature = "body-buffering")))]
extern crate pingora_load_balancing_upstream as pingora_load_balancing;
#[cfg(all(feature = "upstream-pingora", not(feature = "body-buffering")))]
extern crate pingora_proxy_upstream as pingora_proxy;

// Mutually exclusive feature invariants. These selections wire incompatible
// backends (multiple `#[global_allocator]` statics, conflicting pingora TLS
// backends), so fail the build early with an intentional message instead of
// emitting confusing downstream errors (e.g. via `--all-features`).
#[cfg(any(
    all(feature = "allocator-jemalloc", feature = "allocator-mimalloc"),
    all(feature = "allocator-jemalloc", feature = "allocator-system"),
    all(feature = "allocator-mimalloc", feature = "allocator-system"),
))]
compile_error!(
    "features `allocator-jemalloc`, `allocator-mimalloc`, and `allocator-system` are mutually exclusive; enable exactly one"
);

#[cfg(all(feature = "boringssl", feature = "rustls"))]
compile_error!("features `boringssl` and `rustls` are mutually exclusive; enable exactly one TLS backend");

#[cfg(all(feature = "body-buffering", feature = "upstream-pingora"))]
compile_error!("features `body-buffering` and `upstream-pingora` are mutually exclusive; enable exactly one");

#[cfg(not(any(feature = "body-buffering", feature = "upstream-pingora")))]
compile_error!("enable exactly one Pingora source feature: `body-buffering` or `upstream-pingora`");

/// Name of the active global memory allocator (selected by the binary's
/// `allocator-*` feature). Reported in startup/version output.
pub fn allocator_name() -> &'static str {
    #[cfg(all(feature = "allocator-jemalloc", not(target_env = "msvc")))]
    {
        "jemalloc"
    }

    #[cfg(feature = "allocator-mimalloc")]
    {
        "mimalloc"
    }

    #[cfg(feature = "allocator-system")]
    {
        "system"
    }

    #[cfg(not(any(
        feature = "allocator-jemalloc",
        feature = "allocator-mimalloc",
        feature = "allocator-system"
    )))]
    {
        "system (default)"
    }
}

/// Stable, line-oriented description of compile-time Gateway capabilities.
///
/// Integration scripts consume this output before starting the process. Keep
/// the format Bash 3.2-friendly and bump the version before incompatible edits.
pub fn build_capabilities() -> &'static str {
    #[cfg(feature = "body-buffering")]
    {
        "format=edgion-build-capabilities/v1\nbody-buffering=enabled\npingora-mode=edgion-fork\n"
    }

    #[cfg(all(feature = "upstream-pingora", not(feature = "body-buffering")))]
    {
        "format=edgion-build-capabilities/v1\nbody-buffering=disabled\npingora-mode=stock\n"
    }

    #[cfg(not(any(feature = "body-buffering", feature = "upstream-pingora")))]
    {
        ""
    }
}

/// Whether this binary was compiled with the Edgion Pingora request-body
/// capture seam. Runtime policy may still disable buffering.
pub const fn body_buffering_compiled() -> bool {
    cfg!(feature = "body-buffering")
}

#[cfg(any(feature = "body-buffering", feature = "upstream-pingora"))]
pub mod api;
#[cfg(any(feature = "body-buffering", feature = "upstream-pingora"))]
pub mod backends;
#[cfg(any(feature = "body-buffering", feature = "upstream-pingora"))]
pub mod cache;
#[cfg(any(feature = "body-buffering", feature = "upstream-pingora"))]
pub mod cli;
#[cfg(any(feature = "body-buffering", feature = "upstream-pingora"))]
pub mod conf_sync;
#[cfg(any(feature = "body-buffering", feature = "upstream-pingora"))]
pub mod config;
#[cfg(any(feature = "body-buffering", feature = "upstream-pingora"))]
pub mod ctx;
#[cfg(any(feature = "body-buffering", feature = "upstream-pingora"))]
pub mod lb;
#[cfg(any(feature = "body-buffering", feature = "upstream-pingora"))]
pub mod link_sys;
#[cfg(any(feature = "body-buffering", feature = "upstream-pingora"))]
pub mod observe;
#[cfg(any(feature = "body-buffering", feature = "upstream-pingora"))]
pub mod plugins;
#[cfg(any(feature = "body-buffering", feature = "upstream-pingora"))]
pub mod request_body;
#[cfg(any(feature = "body-buffering", feature = "upstream-pingora"))]
pub mod routes;
#[cfg(any(feature = "body-buffering", feature = "upstream-pingora"))]
pub mod runtime;
#[cfg(any(feature = "body-buffering", feature = "upstream-pingora"))]
pub mod services;
#[cfg(any(feature = "body-buffering", feature = "upstream-pingora"))]
pub mod tls;
#[cfg(any(feature = "body-buffering", feature = "upstream-pingora"))]
pub mod util;

#[cfg(any(feature = "body-buffering", feature = "upstream-pingora"))]
pub use ctx::{
    extract_log_labels, BackendContext, BackendTlsInfo, DirectEndpointPreset, EdgionHttpContext, ExternalJumpPreset,
    InternalJumpPreset, LogLabels, MatchInfo, MatchedInfo, MirrorConfig, MirrorState, PeerTimeoutSource,
    PendingHeaderOp, RequestInfo, Rsc, SessionPersistenceState, SetJumpOutcome, TlsConnId, TlsConnMeta, UpstreamInfo,
    UpstreamJumpTarget,
};

#[cfg(any(feature = "body-buffering", feature = "upstream-pingora"))]
pub use cli::EdgionGatewayCli;
#[cfg(any(feature = "body-buffering", feature = "upstream-pingora"))]
pub(crate) use runtime::send_grpc_trailers_only;
#[cfg(any(feature = "body-buffering", feature = "upstream-pingora"))]
pub use runtime::{
    end_response_400, end_response_404, end_response_421, end_response_500, end_response_502, end_response_503,
    end_response_grpc_unavailable, end_response_grpc_unimplemented, terminate_with_fallback,
};
