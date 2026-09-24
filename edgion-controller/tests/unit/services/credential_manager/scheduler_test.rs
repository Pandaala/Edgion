use super::*;

#[test]
fn usable_lifetime_reserves_margin_and_requires_ten_seconds() {
    assert_eq!(usable_lifetime(Duration::from_secs(40)), Some(Duration::from_secs(10)));
    assert_eq!(usable_lifetime(Duration::from_secs(39)), None);
}

#[test]
fn refresh_uses_earlier_of_half_life_and_rotation_with_earlier_only_jitter() {
    let half_life_base = Duration::from_secs(300);
    let delay = refresh_delay("ops/source-a/uid", Duration::from_secs(600), Duration::from_secs(900));
    assert!(delay <= half_life_base);
    assert!(delay >= Duration::from_secs(270));

    let rotation_base = Duration::from_secs(60);
    let rotated = refresh_delay("ops/source-a/uid", Duration::from_secs(600), rotation_base);
    assert!(rotated <= rotation_base);
    assert!(rotated >= Duration::from_secs(54));
    assert_eq!(
        rotated,
        refresh_delay("ops/source-a/uid", Duration::from_secs(600), rotation_base),
        "jitter must be stable for one immutable source identity"
    );
}

#[test]
fn retry_is_exponential_and_retry_after_is_bounded() {
    let initial = Duration::from_secs(2);
    let maximum = Duration::from_secs(20);
    let first = retry_delay("source", 1, initial, maximum, None);
    assert!((Duration::from_secs(1)..=Duration::from_secs(2)).contains(&first));
    let fourth = retry_delay("source", 4, initial, maximum, None);
    assert!((Duration::from_secs(8)..=Duration::from_secs(16)).contains(&fourth));
    let capped = retry_delay("source", 8, initial, maximum, None);
    assert!((Duration::from_secs(10)..=maximum).contains(&capped));
    assert_eq!(
        retry_delay("source", 1, initial, maximum, Some(Duration::from_secs(12))),
        Duration::from_secs(12)
    );
    assert_eq!(
        retry_delay("source", 1, initial, maximum, Some(Duration::from_secs(120))),
        maximum
    );
}
