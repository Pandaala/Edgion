fn main() -> Result<(), Box<dyn std::error::Error>> {
    let allocator_feature_count = [
        "CARGO_FEATURE_ALLOCATOR_JEMALLOC",
        "CARGO_FEATURE_ALLOCATOR_MIMALLOC",
        "CARGO_FEATURE_ALLOCATOR_SYSTEM",
    ]
    .into_iter()
    .filter(|feature| std::env::var_os(feature).is_some())
    .count();
    if allocator_feature_count > 1 {
        return Err("features `allocator-jemalloc`, `allocator-mimalloc`, and `allocator-system` are mutually exclusive; enable exactly one".into());
    }

    // Fail fast on the mutually exclusive TLS backend selection. The lib-level
    // `compile_error!` is a backstop, but the upstream pingora crates also fail
    // when both backends are on; aborting here surfaces a clear, intentional
    // message before that downstream noise appears.
    if std::env::var_os("CARGO_FEATURE_BORINGSSL").is_some() && std::env::var_os("CARGO_FEATURE_RUSTLS").is_some() {
        return Err("features `boringssl` and `rustls` are mutually exclusive; enable exactly one TLS backend".into());
    }

    Ok(())
}
