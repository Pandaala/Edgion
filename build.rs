fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());

    // Proto files are located in conf_sync model
    let proto_dir = "src/core/common/conf_sync/proto";

    // Compile config_sync.proto
    tonic_build::configure()
        .extern_path(".conf_sync.ResourceKind", "crate::types::ResourceKind")
        .file_descriptor_set_path(out_dir.join("config_sync_descriptor.bin"))
        .compile_protos(&[format!("{}/config_sync.proto", proto_dir)], &[proto_dir])?;

    let fed_proto_dir = "src/core/common/fed_sync/proto";
    tonic_build::configure()
        .file_descriptor_set_path(out_dir.join("fed_sync_descriptor.bin"))
        .compile_protos(&[format!("{}/fed_sync.proto", fed_proto_dir)], &[fed_proto_dir])?;

    Ok(())
}
