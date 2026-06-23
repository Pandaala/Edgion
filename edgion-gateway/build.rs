fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Fail fast on the mutually exclusive TLS backend selection. The lib-level
    // `compile_error!` is a backstop, but the upstream pingora crates also fail
    // when both backends are on; aborting here surfaces a clear, intentional
    // message before that downstream noise appears.
    if std::env::var_os("CARGO_FEATURE_BORINGSSL").is_some() && std::env::var_os("CARGO_FEATURE_RUSTLS").is_some() {
        return Err("features `boringssl` and `rustls` are mutually exclusive; enable exactly one TLS backend".into());
    }

    Ok(())
}
