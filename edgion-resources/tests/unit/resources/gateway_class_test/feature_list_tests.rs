use super::{SUPPORTED_FEATURES, UNSUPPORTED_OR_SKIPPED_FEATURES};
use std::collections::BTreeSet;

/// Relative path (from this crate's manifest dir) to the cross-language
/// canonical list the Go conformance suite reads.
const SHARED_LIST_PATH: &str = "../edgion-tests/conformance/supported_features.txt";

fn assert_sorted_unique(list: &[&str], label: &str) {
    for pair in list.windows(2) {
        assert!(
            pair[0] < pair[1],
            "{label} must be sorted ascending with no duplicates, but {:?} is not strictly before {:?}",
            pair[0],
            pair[1]
        );
    }
}

#[test]
fn supported_features_sorted_and_unique() {
    assert_sorted_unique(SUPPORTED_FEATURES, "SUPPORTED_FEATURES");
}

#[test]
fn unsupported_features_sorted_and_unique() {
    assert_sorted_unique(UNSUPPORTED_OR_SKIPPED_FEATURES, "UNSUPPORTED_OR_SKIPPED_FEATURES");
}

#[test]
fn supported_and_unsupported_are_disjoint() {
    let supported: BTreeSet<&str> = SUPPORTED_FEATURES.iter().copied().collect();
    let unsupported: BTreeSet<&str> = UNSUPPORTED_OR_SKIPPED_FEATURES.iter().copied().collect();
    let overlap: Vec<&str> = supported.intersection(&unsupported).copied().collect();
    assert!(
        overlap.is_empty(),
        "a feature may be declared supported OR unsupported, never both: {overlap:?}"
    );
}

/// The Go conformance suite reads `supported_features.txt` to derive its
/// declared feature set. This guards that the committed file is byte-equal
/// to what the controller advertises, so the two never silently diverge.
/// If this fails, regenerate the file from `SUPPORTED_FEATURES`.
#[test]
fn shared_list_file_matches_supported_features() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/",
        "../edgion-tests/conformance/supported_features.txt"
    );
    let raw = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read shared feature list at {SHARED_LIST_PATH}: {e}"));
    let from_file: Vec<&str> = raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    assert_eq!(
        from_file, SUPPORTED_FEATURES,
        "{SHARED_LIST_PATH} is out of sync with SUPPORTED_FEATURES; regenerate it"
    );
}
