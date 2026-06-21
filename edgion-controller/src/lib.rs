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

pub mod api;
pub mod authz;
pub mod cli;
pub mod conf_mgr;
pub mod conf_sync;
pub mod config;
pub mod fed_sync;
pub mod observe;
pub mod services;

pub use cli::EdgionControllerCli;
