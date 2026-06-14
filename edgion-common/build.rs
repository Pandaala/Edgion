fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());

    // conf_sync: Controller <-> Gateway configuration sync.
    let proto_dir = "src/conf_sync/proto";
    tonic_build::configure()
        // `.conf_sync.ResourceKind` has no message in the proto today, so this
        // extern mapping is inert; point it at the real type for correctness.
        .extern_path(
            ".conf_sync.ResourceKind",
            "edgion_resources::resource::kind::ResourceKind",
        )
        .file_descriptor_set_path(out_dir.join("config_sync_descriptor.bin"))
        .compile_protos(&[format!("{}/config_sync.proto", proto_dir)], &[proto_dir])?;

    // ext_proc: Gateway -> external processing service (Envoy-compatible).
    // The gateway only uses the client; the server stub is generated too so the
    // integration test harness can stand up a fake processor.
    let ext_proc_dir = "src/ext_proc/proto";
    tonic_build::configure()
        .file_descriptor_set_path(out_dir.join("ext_proc_descriptor.bin"))
        .compile_protos(&[format!("{}/ext_proc.proto", ext_proc_dir)], &[ext_proc_dir])?;

    Ok(())
}
