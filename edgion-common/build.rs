fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = std::path::PathBuf::from(
        std::env::var_os("OUT_DIR").ok_or("OUT_DIR is not set; run this build script through Cargo")?,
    );

    // conf_sync: Controller <-> Gateway configuration sync.
    let proto_dir = "src/conf_sync/proto";
    tonic_prost_build::configure()
        .bytes(".conf_sync.ListResponse.data")
        .bytes(".conf_sync.WatchResponse.data")
        // `.conf_sync.ResourceKind` has no message in the proto today, so this
        // extern mapping is inert; point it at the real type for correctness.
        .extern_path(
            ".conf_sync.ResourceKind",
            "edgion_resources::resource::kind::ResourceKind",
        )
        .file_descriptor_set_path(out_dir.join("config_sync_descriptor.bin"))
        .compile_protos(&[format!("{}/config_sync.proto", proto_dir)], &[proto_dir.to_string()])?;

    // ext_proc: Gateway -> external processing service (Envoy-compatible).
    // The gateway only uses the client; the server stub is generated too so the
    // integration test harness can stand up a fake processor.
    let ext_proc_dir = "src/ext_proc/proto";
    tonic_prost_build::configure()
        .file_descriptor_set_path(out_dir.join("ext_proc_descriptor.bin"))
        .compile_protos(
            &[format!("{}/ext_proc.proto", ext_proc_dir)],
            &[ext_proc_dir.to_string()],
        )?;

    Ok(())
}
