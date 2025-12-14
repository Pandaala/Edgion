fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());

    // Proto files are located in conf_sync model
    let proto_dir = "src/core/conf_sync/proto";

    // Compile config_sync.proto
    tonic_build::configure()
        .extern_path(".conf_sync.ResourceKind", "crate::types::ResourceKind")
        .file_descriptor_set_path(out_dir.join("config_sync_descriptor.bin"))
        .compile_protos(&[format!("{}/config_sync.proto", proto_dir)], &[proto_dir])?;

    // Compile test_service.proto for examples
    let test_proto_dir = "examples/proto";
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&[format!("{}/test_service.proto", test_proto_dir)], &[test_proto_dir])?;

    Ok(())
}
