fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());

    // fed_sync: Controller <-> Center federation client wire contract.
    // The controller is the federation client; the server side lives in the
    // separate EdgionCenter repository.
    let fed_proto_dir = "src/fed_sync/proto";
    tonic_build::configure()
        .file_descriptor_set_path(out_dir.join("fed_sync_descriptor.bin"))
        .compile_protos(&[format!("{}/fed_sync.proto", fed_proto_dir)], &[fed_proto_dir])?;

    Ok(())
}
