use super::*;

#[test]
fn local_failure_retains_candidate_and_count() {
    let mut r = EndpointRotator::new(vec!["a".into(), "b".into()]);
    r.begin_search(0);
    let candidate = r.candidate();
    r.begin_search(0);
    assert_eq!(r.candidate(), candidate);
    assert_eq!(r.failures, 1);
    assert_eq!(candidate.0, 1);
    assert!(r.take_lap_delay().is_none());
}

#[test]
fn two_failed_laps_back_off_once_each_and_cap() {
    let mut r = EndpointRotator::new(vec!["a".into(), "b".into()]);
    r.begin_search(0);
    r.note_failure();
    assert_eq!(r.candidate().0, 0);
    let first = r.take_lap_delay().unwrap();
    assert!((Duration::from_millis(500)..=Duration::from_millis(1000)).contains(&first));
    assert!(r.take_lap_delay().is_none());
    r.note_failure();
    assert!(r.take_lap_delay().is_none());
    r.note_failure();
    let second = r.take_lap_delay().unwrap();
    assert!((Duration::from_millis(1000)..=Duration::from_millis(2000)).contains(&second));
    assert_eq!(EndpointRotator::lap_cap_ms(100), 30_000);
    r.note_success();
    assert_eq!(r.failures, 0);
    assert!(r.take_lap_delay().is_none());
}

#[test]
fn successful_probe_does_not_reset_meta_failures() {
    let mut r = EndpointRotator::new(vec!["a".into(), "b".into()]);
    r.begin_search(0);
    let ticket = r.candidate();
    r.installed();
    assert!(!r.matches(ticket.0, ticket.1));
    r.begin_search(1);
    assert!(r.take_lap_delay().is_some(), "failed metadata completed a lap");
}

#[test]
fn shuffle_preserves_addresses_and_single_stays_single() {
    let addrs: Vec<String> = (0..8).map(|i| format!("addr-{i}:1")).collect();
    let r = EndpointRotator::new(addrs.clone());
    let mut seen = r.addrs;
    seen.sort();
    assert_eq!(seen, addrs);
    let r = EndpointRotator::new(vec!["a".into()]);
    assert!(r.is_single());
    assert_eq!(r.address(0), "a");
}
