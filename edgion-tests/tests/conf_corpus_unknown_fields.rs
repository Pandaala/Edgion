//! Corpus sweep: walk every YAML fixture under `edgion-tests/integration/conf/` and run the
//! `edgion_resources::schema_check` unknown-key walker against every `EdgionPlugins` /
//! `EdgionStreamPlugins` document's `spec`.
//!
//! This is Phase 3 of `tasks/todo/plugin-config-unknown-field-detection/03-subtasks.md`. It
//! replaces the throwaway prototype `edgion-resources/tests/tmp_corpus_smoke.rs`, which covered
//! only `EdgionPlugins`; this test also covers `EdgionStreamPlugins` (`05-issues.md` R-6).
//!
//! No stack, no ports, no processes — this is a plain, fast `cargo test`.

use std::path::{Path, PathBuf};

use edgion_resources::resources::edgion_plugins::edgion_plugins_spec_schema;
use edgion_resources::resources::edgion_stream_plugins::edgion_stream_plugins_spec_schema;
use edgion_resources::schema_check::unknown_key_paths;

/// Path prefixes (relative to `edgion-tests/integration/conf/`) excluded from the sweep, each
/// with a one-line reason. Reserved for fixtures that are deliberately malformed (negative
/// tests) — see `05-issues.md` R-8. Phase 6 will add `Diagnostics/UnknownFields/` here for its
/// negative-test fixtures; keep this list empty otherwise so a new exclusion stays deliberate
/// and visible in review.
const EXCLUDED_PREFIXES: &[(&str, &str)] = &[
    // Empty by design. Deliberately-malformed fixtures do NOT belong in this
    // corpus at all: run_integration.sh loads the whole tree into a live
    // controller, so a bad fixture here is applied as real config and trips the
    // suite's own pre-flight guard. They live in
    // edgion-tests/integration/fixtures/unknown-config-fields/ instead.
    //
    // Add an entry only for a fixture that must stay in the loaded corpus AND
    // is expected to carry unknown keys, with a one-line reason:
    //     ("Some/Path/", "reason this fixture legitimately carries unknown keys"),
];

fn is_excluded(rel_path: &Path) -> bool {
    let rel_str = rel_path.to_string_lossy();
    EXCLUDED_PREFIXES
        .iter()
        .any(|(prefix, _reason)| rel_str.starts_with(prefix))
}

fn collect_yaml_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("failed to read dir {}: {e}", dir.display()))
        .flatten()
    {
        let path = entry.path();
        if path.is_dir() {
            collect_yaml_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "yaml" || ext == "yml") {
            out.push(path);
        }
    }
}

/// One unknown-field finding, with enough context to report and to allow exclusion filtering.
struct Finding {
    /// Path relative to the `conf/` root, e.g. `EdgionPlugins/HeaderCertAuth/foo.yaml`.
    rel_path: PathBuf,
    /// Dotted key path within the document's `spec`, e.g. `requestPlugins[0].config.verifyDepth`.
    key_path: String,
}

/// What one sweep looked at, so the test can prove it actually inspected the
/// corpus. A sweep that silently matches nothing — the corpus moved, the kind
/// strings changed — would otherwise pass forever while checking nothing.
struct SweepResult {
    findings: Vec<Finding>,
    http_docs: usize,
    stream_docs: usize,
}

fn sweep() -> SweepResult {
    let conf_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("integration/conf");
    assert!(
        conf_root.is_dir(),
        "expected integration conf corpus at {}",
        conf_root.display()
    );

    let mut files = Vec::new();
    collect_yaml_files(&conf_root, &mut files);
    files.sort();

    let plugins_schema = edgion_plugins_spec_schema();
    let stream_plugins_schema = edgion_stream_plugins_spec_schema();

    let mut findings = Vec::new();
    let (mut http_docs, mut stream_docs) = (0usize, 0usize);
    for file in &files {
        let rel_path = file
            .strip_prefix(&conf_root)
            .expect("file was collected under conf_root")
            .to_path_buf();
        if is_excluded(&rel_path) {
            continue;
        }

        let text = std::fs::read_to_string(file).unwrap_or_else(|e| panic!("failed to read {}: {e}", file.display()));

        for doc in serde_yaml::Deserializer::from_str(&text) {
            let value: serde_json::Value = match serde::Deserialize::deserialize(doc) {
                Ok(v) => v,
                // Non-resource / non-YAML-object documents (e.g. an empty `---` separator) are
                // not this sweep's concern.
                Err(_) => continue,
            };
            let Some(kind) = value.get("kind").and_then(|k| k.as_str()) else {
                continue;
            };
            let schema = match kind {
                "EdgionPlugins" => {
                    http_docs += 1;
                    plugins_schema
                }
                "EdgionStreamPlugins" => {
                    stream_docs += 1;
                    stream_plugins_schema
                }
                _ => continue,
            };
            let Some(spec) = value.get("spec") else { continue };

            for key_path in unknown_key_paths(spec, schema) {
                findings.push(Finding {
                    rel_path: rel_path.clone(),
                    key_path,
                });
            }
        }
    }

    SweepResult {
        findings,
        http_docs,
        stream_docs,
    }
}

#[test]
fn conf_corpus_has_no_unknown_plugin_config_fields() {
    let SweepResult {
        findings,
        http_docs,
        stream_docs,
    } = sweep();

    // Coverage floors, deliberately far below the real corpus size (300+ HTTP,
    // 20+ stream documents at the time of writing). They exist to fail loudly
    // if this sweep ever stops matching documents at all, rather than passing
    // vacuously.
    assert!(
        http_docs >= 100,
        "sweep matched only {http_docs} EdgionPlugins documents — the corpus moved, \
         the kind string changed, or the exclusion list swallowed it"
    );
    assert!(
        stream_docs >= 5,
        "sweep matched only {stream_docs} EdgionStreamPlugins documents — the corpus moved, \
         the kind string changed, or the exclusion list swallowed it"
    );

    if findings.is_empty() {
        return;
    }

    let mut lines: Vec<String> = findings
        .iter()
        .map(|f| format!("{}: {}", f.rel_path.display(), f.key_path))
        .collect();
    lines.sort();

    panic!(
        "unknown-field findings in the integration conf corpus ({} total):\n{}\n\n\
         Each finding is either a stale/renamed key (fix the fixture), a key the fixture's \
         author believed was active (re-check the consuming integration test's expectation), \
         or a genuine schema/capability gap (do NOT invent the field — split into its own task). \
         See tasks/todo/plugin-config-unknown-field-detection/03-subtasks.md (Phase 3) and \
         05-issues.md (R-8) for the triage process and the exclusion-list mechanism.",
        lines.len(),
        lines.join("\n")
    );
}
