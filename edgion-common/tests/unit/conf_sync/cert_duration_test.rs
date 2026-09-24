use super::*;

#[test]
fn parses_all_units() {
    assert_eq!(CertDuration::parse("1h").unwrap().as_std().as_secs(), 3600);
    assert_eq!(CertDuration::parse("2d").unwrap().as_std().as_secs(), 2 * 24 * 3600);
    assert_eq!(CertDuration::parse("1w").unwrap().as_std().as_secs(), 7 * 24 * 3600);
    assert_eq!(CertDuration::parse("1mo").unwrap().as_std().as_secs(), 30 * 24 * 3600);
    assert_eq!(CertDuration::parse("1y").unwrap().as_std().as_secs(), 365 * 24 * 3600);
}

#[test]
fn rejects_bad_inputs() {
    for bad in [
        "",
        "12",
        "h",
        "0h",
        "0d",
        "-1h",
        "1.5h",
        "1y6mo",
        "1s",
        "1m",
        "99999999999999999999h",
    ] {
        assert!(CertDuration::parse(bad).is_err(), "should reject {bad:?}");
    }
    // Overflow via checked mul: u64::MAX hours overflows seconds.
    assert!(CertDuration::parse(&format!("{}h", u64::MAX)).is_err());
}

#[test]
fn rejects_hours_whose_seconds_exceed_i64_max_but_fit_u64() {
    // `h * 3600` fits comfortably in `u64` (the old bound) but exceeds
    // `i64::MAX` (the tightened bound `as_time()` actually needs, since it
    // casts `hours` to `i64` and `time::Duration::hours` multiplies by 3600
    // in `i64` space). This is the smallest `h` in that gap.
    let h = (i64::MAX as u64) / 3600 + 1;
    assert!(
        h.checked_mul(3600).is_some(),
        "sanity: h*3600 must still fit in u64 for this to test the intended gap"
    );
    assert!(
        h.checked_mul(3600).unwrap() > i64::MAX as u64,
        "sanity: h*3600 must exceed i64::MAX for this to test the intended gap"
    );
    assert!(CertDuration::parse(&format!("{h}h")).is_err());
}

#[test]
fn serde_round_trip() {
    let d: CertDuration = serde_json::from_str("\"36h\"").unwrap();
    assert_eq!(d.as_std().as_secs(), 36 * 3600);
    assert_eq!(serde_json::to_string(&d).unwrap(), "\"36h\"");
    assert!(serde_json::from_str::<CertDuration>("\"0h\"").is_err());
    assert!(serde_json::from_str::<CertDuration>("36").is_err()); // bare integer rejected
}
