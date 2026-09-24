use super::*;

#[test]
fn parse_fips_enabled_accepts_only_one() {
    assert!(parse_fips_enabled("1"));
    assert!(parse_fips_enabled("1\n"));
    assert!(parse_fips_enabled("  1  "));
    assert!(!parse_fips_enabled("0"));
    assert!(!parse_fips_enabled(""));
    assert!(!parse_fips_enabled("abc"));
    assert!(!parse_fips_enabled("11"));
}

#[test]
fn detect_at_reads_enabled_and_disabled() {
    let dir = tempfile::tempdir().unwrap();

    let on = dir.path().join("on");
    std::fs::write(&on, "1\n").unwrap();
    assert_eq!(detect_host_fips_at(&on), HostFipsState::Enabled);

    let off = dir.path().join("off");
    std::fs::write(&off, "0\n").unwrap();
    assert_eq!(detect_host_fips_at(&off), HostFipsState::Disabled);
}

#[test]
fn detect_at_missing_path_is_disabled_not_unknown() {
    // The common case: a non-FIPS kernel, or any non-Linux host, simply has
    // no such file. That is a definite "not a FIPS host", not an error.
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("definitely-absent");
    assert_eq!(detect_host_fips_at(&missing), HostFipsState::Disabled);
}

#[test]
fn detect_at_unreadable_path_is_unknown() {
    // A path that exists but cannot be read as a file (here: a directory,
    // which yields EISDIR). Distinguished from "absent" because a hardened
    // node that masks /proc must not look like a plain non-FIPS host.
    //
    // A directory is used rather than a 0o000-mode file on purpose: CI
    // containers frequently run as root, where mode bits are bypassed and
    // the read would succeed, silently asserting the wrong branch.
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(detect_host_fips_at(dir.path()), HostFipsState::Unknown);
}

#[test]
fn mismatch_fires_only_on_enabled_host_without_module() {
    assert!(host_fips_mismatch(HostFipsState::Enabled, false));
    assert!(!host_fips_mismatch(HostFipsState::Enabled, true));
    assert!(!host_fips_mismatch(HostFipsState::Disabled, false));
    // Unknown must not fire the posture guardrail: we could not tell.
    assert!(!host_fips_mismatch(HostFipsState::Unknown, false));
}
