// Standalone program to generate test_service proto code
// 
// Usage:
//   cd examples/testing
//   cargo run --manifest-path ../../Cargo.toml --bin build_test_proto
//
// This generates proto_gen/test.rs from ../proto/test_service.proto

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = "examples/testing/proto_gen";
    std::fs::create_dir_all(out_dir)?;
    
    println!("Generating test_service proto code...");
    println!("  Input: examples/proto/test_service.proto");
    println!("  Output: {}/test.rs", out_dir);
    
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .out_dir(out_dir)
        .compile_protos(
            &["examples/proto/test_service.proto"], 
            &["examples/proto"]
        )?;
    
    println!("✓ Successfully generated proto code!");
    println!("\nGenerated file:");
    let generated_file = format!("{}/test.rs", out_dir);
    if let Ok(metadata) = std::fs::metadata(&generated_file) {
        println!("  {} ({} bytes)", generated_file, metadata.len());
    }
    
    println!("\nNext steps:");
    println!("  1. Review the generated code: {}/test.rs", out_dir);
    println!("  2. Commit it to git");
    println!("  3. Update examples to use #[path = \"proto_gen/test.rs\"]");
    
    Ok(())
}

