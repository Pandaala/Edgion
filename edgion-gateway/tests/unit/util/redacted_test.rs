use super::*;

#[test]
fn debug_never_prints_the_value() {
    let s: RedactedString = "super-secret-token".into();
    let dbg = format!("{:?}", s);
    assert_eq!(dbg, "[redacted]");
    assert!(!dbg.contains("super-secret-token"));
}

#[test]
fn debug_of_none_shows_no_secret_either() {
    let none: Option<RedactedString> = None;
    let some: Option<RedactedString> = Some("tok".into());
    assert_eq!(format!("{:?}", none), "None");
    assert_eq!(format!("{:?}", some), "Some([redacted])");
}

#[test]
fn display_and_deref_expose_the_real_value() {
    let s: RedactedString = "https://mirror.internal/db.mmdb?token=abc".into();
    assert_eq!(s.to_string(), "https://mirror.internal/db.mmdb?token=abc");
    assert_eq!(&*s, "https://mirror.internal/db.mmdb?token=abc");
    assert_eq!(s.as_str(), "https://mirror.internal/db.mmdb?token=abc");
}

#[test]
fn as_deref_on_option_yields_a_str() {
    let opt: Option<RedactedString> = Some("v".into());
    let d: Option<&str> = opt.as_deref();
    assert_eq!(d, Some("v"));
}

#[test]
fn serde_round_trips_transparently() {
    let s: RedactedString = "value".into();
    let json = serde_json::to_string(&s).unwrap();
    assert_eq!(json, "\"value\"");
    let back: RedactedString = serde_json::from_str(&json).unwrap();
    assert_eq!(back.as_str(), "value");
}
