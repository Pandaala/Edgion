// Standalone program to generate test_service proto code
//
// Usage:
//   cargo run --example build_test_proto
//
// This generates proto_gen/test.rs from examples/proto/test_service.proto

fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::path::Path;

    let out_dir = "examples/code/proto_gen";
    std::fs::create_dir_all(out_dir)?;

    let proto_candidates = [
        "examples/proto/test_service.proto",
        "examples/test/proto/test_service.proto",
        "examples/code/proto/test_service.proto",
    ];
    let proto_path = proto_candidates.iter().find(|p| Path::new(*p).exists()).copied();
    let generated_file = format!("{}/test.rs", out_dir);

    let Some(proto_path) = proto_path else {
        if Path::new(&generated_file).exists() {
            println!("⚠ test_service.proto not found, keep existing generated file: {}", generated_file);
            return Ok(());
        }
        return Err(
            "test_service.proto not found (checked: examples/proto, examples/test/proto, examples/code/proto)"
                .into(),
        );
    };
    let include_dir = Path::new(proto_path)
        .parent()
        .and_then(|p| p.to_str())
        .ok_or("invalid proto include dir")?;

    println!("Generating test_service proto code...");
    println!("  Input: {}", proto_path);
    println!("  Output: {}/test.rs", out_dir);

    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .out_dir(out_dir)
        .compile_protos(&[proto_path], &[include_dir])?;

    println!("✓ Successfully generated proto code!");
    println!("\nGenerated file:");
    if let Ok(metadata) = std::fs::metadata(&generated_file) {
        println!("  {} ({} bytes)", generated_file, metadata.len());
    }

    println!("\nNext steps:");
    println!("  1. Review the generated code: {}/test.rs", out_dir);
    println!("  2. Commit it to git");
    println!("  3. Update examples to use #[path = \"proto_gen/test.rs\"]");

    Ok(())
}
