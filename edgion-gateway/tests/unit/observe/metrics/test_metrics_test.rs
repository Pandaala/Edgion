use super::*;

#[test]
fn test_type_parsing() {
    assert_eq!(TestType::from_str("lb"), TestType::Lb);
    assert_eq!(TestType::from_str("LB"), TestType::Lb);
    assert_eq!(TestType::from_str("Lb"), TestType::Lb);
    assert_eq!(TestType::from_str("retry"), TestType::Retry);
    assert_eq!(TestType::from_str("RETRY"), TestType::Retry);
    assert_eq!(TestType::from_str("latency"), TestType::Latency);
    assert_eq!(TestType::from_str("LATENCY"), TestType::Latency);
    assert_eq!(TestType::from_str("unknown"), TestType::None);
    assert_eq!(TestType::from_str(""), TestType::None);
}

#[test]
fn test_type_is_enabled() {
    assert!(!TestType::None.is_enabled());
    assert!(TestType::Lb.is_enabled());
    assert!(TestType::Retry.is_enabled());
    assert!(TestType::Latency.is_enabled());
}

#[test]
fn test_empty_data() {
    let data = TestData::new();
    assert!(data.is_empty());
    assert_eq!(data.to_json(), "");
}
