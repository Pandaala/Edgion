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

#[cfg(test)]
#[path = "../tests/unit/lib_test/test_current_status.rs"]
pub(crate) mod test_current_status;

#[cfg(test)]
#[path = "../tests/unit/lib_test/test_allocation_counter.rs"]
mod test_allocation_counter;

#[cfg(test)]
include!("../tests/support/lib_test/25_static_item.rs");

/// Name of the active global memory allocator (selected by the binary's
/// `allocator-*` feature). Reported in startup/version output.
pub fn allocator_name() -> &'static str {
    #[cfg(all(feature = "allocator-jemalloc", not(target_env = "msvc")))]
    {
        "jemalloc"
    }

    #[cfg(all(feature = "allocator-jemalloc", target_env = "msvc"))]
    {
        "system (default)"
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
/// The Gateway always builds against the Edgion Pingora fork, so the body
/// capture seam is unconditionally present.
pub fn build_capabilities() -> &'static str {
    "format=edgion-build-capabilities/v1\nbody-buffering=enabled\npingora-mode=edgion-fork\n"
}

pub mod ai;
pub mod api;
pub mod backends;
pub mod cache;
pub mod cli;
pub mod conf_sync;
pub mod config;
pub mod ctx;
pub mod geoip;
pub mod lb;
pub mod link_sys;
pub mod observe;
pub mod plugins;
pub mod request_body;
mod response;
pub mod routes;
pub mod runtime;
pub mod services;
pub mod sse;
pub mod tls;
pub mod util;

pub use ctx::{
    extract_log_labels, BackendContext, BackendTlsInfo, DirectEndpointPreset, EdgionHttpContext, ExternalJumpPreset,
    InternalJumpPreset, LogLabels, MatchInfo, MatchedInfo, MirrorConfig, MirrorState, PeerTimeoutSource,
    PendingHeaderOp, RequestInfo, ResponseStageDenial, Rsc, SessionPersistenceState, SetJumpOutcome, TlsConnId,
    TlsConnMeta, UpstreamInfo, UpstreamJumpTarget,
};

pub use cli::EdgionGatewayCli;
pub(crate) use runtime::send_grpc_trailers_only;
pub use runtime::{
    end_response_400, end_response_404, end_response_421, end_response_500, end_response_502, end_response_503,
    end_response_grpc_unavailable, end_response_grpc_unimplemented, terminate_with_fallback,
};

pub mod request_trace_id;

#[cfg(test)]
#[path = "../tests/unit/lib_test/request_id_tests.rs"]
mod request_id_tests;

#[cfg(test)]
#[path = "../tests/unit/lib_test/test_subprocess.rs"]
mod test_subprocess;
