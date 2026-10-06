use super::*;
#[test]
fn defaults_derive_reviewed_reservation_and_slots() {
    let l = UnaryLimits::resolve(&UnaryConfig::default()).unwrap();
    assert_eq!(l.reservation, 25 * MIB);
    assert_eq!(l.slots, 10);
    assert_eq!(l.lexical_events, 49152);
    assert_eq!(l.lexical_work, 4 * MIB);
    eprintln!("unary pinned layout: pointer={}, Value={}, Value_align={}, String={}, Bytes={}, HttpTask={}, HeaderName={}, HeaderValue={}, Number={}, Map={}; reservation={}, slots={}",std::mem::size_of::<usize>(),std::mem::size_of::<serde_json::Value>(),std::mem::align_of::<serde_json::Value>(),std::mem::size_of::<String>(),std::mem::size_of::<bytes::Bytes>(),std::mem::size_of::<pingora_core::protocols::http::HttpTask>(),std::mem::size_of::<http::HeaderName>(),std::mem::size_of::<http::HeaderValue>(),std::mem::size_of::<serde_json::Number>(),std::mem::size_of::<serde_json::Map<String,serde_json::Value>>(),l.reservation,l.slots);
}
#[test]
fn invalid_operational_limits_fail_resolution() {
    for value in ["0B", "17MiB", "18446744073709551615GiB", "invalid"] {
        let c = UnaryConfig {
            max_input_size: value.into(),
            ..Default::default()
        };
        assert!(UnaryLimits::resolve(&c).is_err());
    }
    for value in ["0s", "1ns", "3601s", "false"] {
        let c = UnaryConfig {
            hold_timeout: value.into(),
            ..Default::default()
        };
        assert!(UnaryLimits::resolve(&c).is_err());
    }
    let mut c = UnaryConfig {
        memory_budget: "24MiB".into(),
        ..Default::default()
    };
    assert!(UnaryLimits::resolve(&c)
        .unwrap_err()
        .contains("26214400-byte reservation"));
    c.memory_budget = "0B".into();
    assert!(UnaryLimits::resolve(&c)
        .unwrap_err()
        .contains("26214400-byte reservation"));
    c = UnaryConfig::default();
    c.max_metadata_size = "4KiB".into();
    assert!(UnaryLimits::resolve(&c).is_err());
    assert!(reservation(usize::MAX, 1, 1, 4, 8192).is_err());
    assert!(reservation(1, 1, 1, usize::MAX, 8192).is_err());
}
#[test]
fn bounded_growth_accepts_equality_and_leaves_overflow_atomic() {
    let mut b = vec![1, 2];
    append(&mut b, &[3, 4], 4).unwrap();
    assert_eq!(b, vec![1, 2, 3, 4]);
    let capacity = b.capacity();
    assert!(append(&mut b, &[5], 4).is_err());
    assert_eq!(b, vec![1, 2, 3, 4]);
    assert_eq!(capacity, b.capacity());
}
