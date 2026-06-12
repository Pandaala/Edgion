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

    // fed_sync: federation client (still used by the controller's fed_sync).
    let fed_proto_dir = "src/fed_sync/proto";
    tonic_build::configure()
        .file_descriptor_set_path(out_dir.join("fed_sync_descriptor.bin"))
        .compile_protos(&[format!("{}/fed_sync.proto", fed_proto_dir)], &[fed_proto_dir])?;

    Ok(())
}
