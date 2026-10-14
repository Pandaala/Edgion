// Explicitly regenerate the checked-in test package from its annotated schema.
// Usage: cargo run -p edgion-tests --bin build_test_proto

use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

fn generate(proto_dir: &Path, out_dir: &Path) -> Result<()> {
    let proto_path = proto_dir.join("test.proto");
    fs::metadata(&proto_path).with_context(|| {
        format!(
            "Cannot read schema {}; restore it before regenerating",
            proto_path.display()
        )
    })?;
    fs::create_dir_all(out_dir).with_context(|| format!("Cannot create output directory {}", out_dir.display()))?;
    // Keep generation and formatting failures away from the checked-in artifact.
    // A same-filesystem rename publishes only the completed test package.
    let staging = tempfile::Builder::new().prefix(".test-proto-").tempdir_in(out_dir)?;
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .out_dir(staging.path())
        .compile_protos(&[proto_path], &[proto_dir])
        .context("Test schema generation failed; check the schema and protoc installation")?;

    let generated_file = staging.path().join("test.rs");
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rustfmt.toml");
    let status = Command::new("rustfmt")
        .args(["--edition", "2021", "--config-path"])
        .arg(config)
        .arg(&generated_file)
        .status()
        .context("Cannot run rustfmt; install the rustfmt component before regenerating")?;
    if !status.success() {
        bail!("rustfmt failed with {status}; the existing test.rs was preserved");
    }
    // Imported annotation definitions are not referenced by the test package's
    // Rust types. Discard their auxiliary output with the staging directory.
    let destination = out_dir.join("test.rs");
    fs::rename(&generated_file, &destination)
        .with_context(|| format!("Cannot publish generated file {}", destination.display()))?;
    Ok(())
}

fn main() -> Result<()> {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let proto_dir = crate_dir.join("proto/grpc_json_transcoding");
    let out_dir = crate_dir.join("src/proto_gen");
    generate(&proto_dir, &out_dir)?;
    println!(
        "Generated {} from {}",
        out_dir.join("test.rs").display(),
        proto_dir.join("test.proto").display()
    );
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/build_test_proto_test.rs"]
mod tests;
