fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-env-changed=BORING_BSSL_FIPS_PATH");

    // Fail fast on the mutually exclusive TLS backend selection. The lib-level
    // `compile_error!` is a backstop, but the upstream pingora crates also fail
    // when both backends are on; aborting here surfaces a clear, intentional
    // message before that downstream noise appears.
    if std::env::var_os("CARGO_FEATURE_BORINGSSL").is_some() && std::env::var_os("CARGO_FEATURE_RUSTLS").is_some() {
        return Err("features `boringssl` and `rustls` are mutually exclusive; enable exactly one TLS backend".into());
    }

    // boring v5 merged its precompiled mode into `fips` and otherwise builds
    // BoringSSL from source. Edgion's FIPS image contract requires the prepared
    // module tree, so retain the old fail-closed behavior explicitly.
    if std::env::var_os("CARGO_FEATURE_FIPS").is_some() {
        let path = std::env::var_os("BORING_BSSL_FIPS_PATH")
            .filter(|value| !value.is_empty())
            .ok_or("feature `fips` requires `BORING_BSSL_FIPS_PATH` to point to a precompiled BoringSSL FIPS tree")?;
        if !std::path::Path::new(&path).is_dir() {
            return Err(
                "`BORING_BSSL_FIPS_PATH` must point to an existing precompiled BoringSSL FIPS directory".into(),
            );
        }
    }

    Ok(())
}
