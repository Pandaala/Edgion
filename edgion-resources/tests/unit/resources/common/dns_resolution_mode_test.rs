use super::*;

#[test]
fn absent_annotation_inherits() {
    assert_eq!(DnsResolutionMode::from_annotation(None), Ok(None));
}

#[test]
fn exact_spellings_parse() {
    assert_eq!(
        DnsResolutionMode::from_annotation(Some("gateway")),
        Ok(Some(DnsResolutionMode::Gateway))
    );
    assert_eq!(
        DnsResolutionMode::from_annotation(Some("system")),
        Ok(Some(DnsResolutionMode::System))
    );
    assert_eq!(
        DnsResolutionMode::from_annotation(Some("controller")),
        Ok(Some(DnsResolutionMode::Controller))
    );
}

#[test]
fn unknown_and_retired_values_fail_closed() {
    for value in ["", "Gateway", " gateway", "per-request", "disabled", "auto"] {
        assert!(
            DnsResolutionMode::from_annotation(Some(value)).is_err(),
            "{value:?} must be rejected"
        );
    }
}

#[test]
fn global_default_follows_endpoint_table_switch() {
    assert_eq!(DnsResolutionMode::global_default(true), DnsResolutionMode::Gateway);
    assert_eq!(DnsResolutionMode::global_default(false), DnsResolutionMode::System);
}

#[test]
fn as_str_round_trips() {
    for mode in [
        DnsResolutionMode::Gateway,
        DnsResolutionMode::System,
        DnsResolutionMode::Controller,
    ] {
        assert_eq!(DnsResolutionMode::from_annotation(Some(mode.as_str())), Ok(Some(mode)));
    }
}
