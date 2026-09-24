use super::*;
use chrono::Duration;
use std::collections::BTreeMap;

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-05-18T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

#[test]
fn age_seconds_under_60() {
    let creation = now() - Duration::seconds(5);
    assert_eq!(format_age(Some(creation), now()), "5s");
}

#[test]
fn age_exactly_60s_rolls_to_1m() {
    let creation = now() - Duration::seconds(60);
    assert_eq!(format_age(Some(creation), now()), "1m");
}

#[test]
fn age_minutes_under_60() {
    let creation = now() - Duration::minutes(45);
    assert_eq!(format_age(Some(creation), now()), "45m");
}

#[test]
fn age_exactly_3600s_rolls_to_1h() {
    let creation = now() - Duration::seconds(3600);
    assert_eq!(format_age(Some(creation), now()), "1h");
}

#[test]
fn age_hours_under_24() {
    let creation = now() - Duration::hours(5);
    assert_eq!(format_age(Some(creation), now()), "5h");
}

#[test]
fn age_exactly_86400s_rolls_to_1d() {
    let creation = now() - Duration::seconds(86400);
    assert_eq!(format_age(Some(creation), now()), "1d");
}

#[test]
fn age_days() {
    let creation = now() - Duration::days(7);
    assert_eq!(format_age(Some(creation), now()), "7d");
}

#[test]
fn age_none_is_unknown() {
    assert_eq!(format_age(None, now()), "<unknown>");
}

#[test]
fn age_negative_duration_is_unknown() {
    let creation = now() + Duration::seconds(10);
    assert_eq!(format_age(Some(creation), now()), "<unknown>");
}

#[test]
fn labels_empty_returns_none_marker() {
    let labels: BTreeMap<String, String> = BTreeMap::new();
    assert_eq!(format_labels(&labels), "<none>");
}

#[test]
fn labels_single_pair() {
    let mut labels = BTreeMap::new();
    labels.insert("app".to_string(), "edgion".to_string());
    assert_eq!(format_labels(&labels), "app=edgion");
}

#[test]
fn labels_multiple_pairs_sorted() {
    let mut labels = BTreeMap::new();
    labels.insert("zone".to_string(), "us-east".to_string());
    labels.insert("app".to_string(), "edgion".to_string());
    labels.insert("env".to_string(), "prod".to_string());
    // BTreeMap sorts: app < env < zone
    assert_eq!(format_labels(&labels), "app=edgion,env=prod,zone=us-east");
}
