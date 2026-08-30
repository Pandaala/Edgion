fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Fail fast on the mutually exclusive TLS backend selection. The lib-level
    // `compile_error!` is a backstop, but the upstream pingora crates also fail
    // when both backends are on; aborting here surfaces a clear, intentional
    // message before that downstream noise appears.
    if std::env::var_os("CARGO_FEATURE_BORINGSSL").is_some() && std::env::var_os("CARGO_FEATURE_RUSTLS").is_some() {
        return Err("features `boringssl` and `rustls` are mutually exclusive; enable exactly one TLS backend".into());
    }

    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());

    // fed_sync: Controller <-> Center federation client wire contract.
    // The controller is the federation client; the server side lives in the
    // separate EdgionCenter repository.
    let fed_proto_dir = "src/fed_sync/proto";
    tonic_build::configure()
        .bytes([".fed_sync.FedWatchListResponse.data"])
        .file_descriptor_set_path(out_dir.join("fed_sync_descriptor.bin"))
        .compile_protos(&[format!("{}/fed_sync.proto", fed_proto_dir)], &[fed_proto_dir])?;

    Ok(())
}
