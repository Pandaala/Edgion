//! Canonical-pattern drift guard for the hand-written edgion CRDs.
//!
//! The CRD YAML under `config/crd/edgion-crd/` cannot reference the Rust
//! constants in `edgion_resources`, so inline regex
//! `pattern:` constraints could silently drift from the canonical grammars.
//! This test parses every committed edgion CRD and asserts each regex pattern
//! is byte-for-byte one of the shared constants. A legitimately new grammar
//! must be added as a shared constant first, then listed here.

use serde_yaml::Value;
use std::fs;
use std::path::PathBuf;

/// Recursively collect every string value stored under a `pattern` key.
///
/// Only string values are regex constraints; a `pattern` key whose value is a
/// mapping is a schema *property named* `pattern` (e.g.
/// EdgionBackendTrafficPolicy's authority-rewrite `pattern` field) and is
/// descended into, not collected.
fn collect_patterns(node: &Value, out: &mut Vec<String>) {
    match node {
        Value::Mapping(map) => {
            for (k, v) in map {
                if k.as_str() == Some("pattern") {
                    if let Some(s) = v.as_str() {
                        out.push(s.to_string());
                        continue;
                    }
                }
                collect_patterns(v, out);
            }
        }
        Value::Sequence(seq) => {
            for v in seq {
                collect_patterns(v, out);
            }
        }
        _ => {}
    }
}

#[test]
fn edgion_crd_patterns_are_canonical() {
    let canonical: &[&str] = &[
        edgion_resources::duration::GEP2257_PATTERN,
        edgion_resources::gwapi_types::HOSTNAME_PATTERN,
        edgion_resources::gwapi_types::PRECISE_HOSTNAME_PATTERN,
        edgion_resources::gwapi_types::HEADER_NAME_PATTERN,
        edgion_resources::gwapi_types::GROUP_NAME_PATTERN,
        edgion_resources::resources::edgion_backend::AI_CREDENTIAL_ALIAS_PATTERN,
        edgion_resources::resources::edgion_backend::AI_REDIS_REF_PATTERN,
        edgion_resources::resources::edgion_backend::AI_PRICE_PATTERN,
        edgion_resources::resources::edgion_backend::KUBE_DNS_LABEL_PATTERN,
    ];

    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/crd/edgion-crd");
    let mut checked_files = 0usize;
    let mut checked_patterns = 0usize;
    let mut violations = Vec::new();

    for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {dir:?}: {e}")) {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }
        checked_files += 1;
        let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
        let doc: Value = serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {path:?}: {e}"));
        let mut patterns = Vec::new();
        collect_patterns(&doc, &mut patterns);
        for pattern in patterns {
            checked_patterns += 1;
            if !canonical.contains(&pattern.as_str()) {
                violations.push(format!("{}: {pattern:?}", path.file_name().unwrap().to_string_lossy()));
            }
        }
    }

    assert!(checked_files > 0, "no CRD YAML found under {dir:?}");
    assert!(checked_patterns > 0, "no regex patterns found — collector broken?");
    assert!(
        violations.is_empty(),
        "non-canonical regex pattern(s) in edgion CRDs — every schema `pattern:` must equal one \
         of the shared constants in edgion_resources. Offending:\n{}",
        violations.join("\n")
    );
}
