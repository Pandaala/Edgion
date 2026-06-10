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

    ensure_embed_dashboard_placeholder();

    Ok(())
}

/// The dashboard UI is NOT vendored in this repo — `web/dashboard/` is gitignored
/// and only populated at image-build time (build-image.sh downloads the
/// edgion-dashboard repo and stages its `dist/` there). But rust-embed's
/// `#[folder = "web/dashboard/"]` must find the folder at compile time. When the
/// `embed-dashboard` feature is on and no real build has been staged, write a
/// minimal placeholder so the feature still compiles standalone (e.g. local
/// `cargo check --features embed-dashboard`). A real staged `index.html` is never
/// overwritten — we only create the placeholder when it is missing.
fn ensure_embed_dashboard_placeholder() {
    if std::env::var_os("CARGO_FEATURE_EMBED_DASHBOARD").is_none() {
        return;
    }
    let dir = std::path::Path::new("web/dashboard");
    let index = dir.join("index.html");
    if index.exists() {
        return;
    }
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(
        &index,
        "<!doctype html>\n<html lang=\"en\">\n  <head><meta charset=\"UTF-8\" />\
         <title>Edgion Center</title></head>\n  <body>\n    <div id=\"root\">\
         Edgion Center dashboard is not built into this binary.</div>\n  </body>\n</html>\n",
    );
}
