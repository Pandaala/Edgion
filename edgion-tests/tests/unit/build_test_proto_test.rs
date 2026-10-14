use super::*;

#[test]
fn missing_schema_preserves_existing_artifact() {
    let temp = tempfile::tempdir().unwrap();
    let out_dir = temp.path().join("output");
    fs::create_dir(&out_dir).unwrap();
    let artifact = out_dir.join("test.rs");
    fs::write(&artifact, "previous artifact").unwrap();

    let error = generate(&temp.path().join("missing"), &out_dir).unwrap_err();
    assert!(error.to_string().contains("Cannot read schema"));
    assert_eq!(fs::read_to_string(artifact).unwrap(), "previous artifact");
}

#[test]
fn invalid_schema_preserves_existing_artifact_and_cleans_staging() {
    let temp = tempfile::tempdir().unwrap();
    let out_dir = temp.path().join("output");
    fs::create_dir(&out_dir).unwrap();
    let artifact = out_dir.join("test.rs");
    fs::write(&artifact, "previous artifact").unwrap();
    fs::write(temp.path().join("test.proto"), "invalid proto syntax").unwrap();

    let error = generate(temp.path(), &out_dir).unwrap_err();
    assert!(error.to_string().contains("Test schema generation failed"));
    assert_eq!(fs::read_to_string(artifact).unwrap(), "previous artifact");
    assert_eq!(fs::read_dir(out_dir).unwrap().count(), 1);
}
