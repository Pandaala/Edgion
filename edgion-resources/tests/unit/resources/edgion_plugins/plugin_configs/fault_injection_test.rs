use super::*;

#[test]
fn serde_roundtrip_full() {
    let json = r#"{
            "delay": { "ratio": 0.5, "fixedMs": 300 },
            "abort": {
                "ratio": 0.25,
                "statusCode": 503,
                "body": "injected",
                "contentType": "text/plain",
                "headers": [{ "name": "X-Fault", "value": "1" }]
            }
        }"#;
    let cfg: FaultInjectionConfig = serde_json::from_str(json).unwrap();
    let d = cfg.delay.as_ref().unwrap();
    assert_eq!(d.ratio, 0.5);
    assert_eq!(d.fixed_ms, Some(300));
    let a = cfg.abort.as_ref().unwrap();
    assert_eq!(a.status_code, 503);
    assert_eq!(a.body.as_deref(), Some("injected"));
    assert_eq!(a.content_type, "text/plain");
    assert_eq!(a.headers.as_ref().unwrap()[0].name, "X-Fault");
    let back = serde_json::to_string(&cfg).unwrap();
    assert!(back.contains("\"fixedMs\":300"));
    assert!(back.contains("\"statusCode\":503"));
}

#[test]
fn defaults_apply() {
    let json = r#"{ "abort": {} }"#;
    let cfg: FaultInjectionConfig = serde_json::from_str(json).unwrap();
    let a = cfg.abort.as_ref().unwrap();
    assert_eq!(a.ratio, 1.0);
    assert_eq!(a.status_code, 500);
    assert_eq!(a.content_type, "text/plain");
    assert!(a.body.is_none());
    assert!(a.headers.is_none());
}

#[test]
fn default_impl_is_empty() {
    let cfg = FaultInjectionConfig::default();
    assert!(cfg.delay.is_none());
    assert!(cfg.abort.is_none());
}

fn cfg_delay_fixed(ms: u64) -> FaultInjectionConfig {
    FaultInjectionConfig {
        delay: Some(DelayConfig {
            ratio: 1.0,
            fixed_ms: Some(ms),
            min_ms: None,
            max_ms: None,
        }),
        abort: None,
    }
}

fn cfg_abort(status: u16) -> FaultInjectionConfig {
    FaultInjectionConfig {
        delay: None,
        abort: Some(AbortConfig {
            ratio: 1.0,
            status_code: status,
            body: None,
            content_type: "text/plain".to_string(),
            headers: None,
        }),
    }
}

#[test]
fn validate_ok_minimal_delay() {
    assert!(cfg_delay_fixed(0).validate().is_ok());
    assert!(cfg_delay_fixed(MAX_DELAY_MS).validate().is_ok());
}

#[test]
fn validate_ok_minimal_abort() {
    assert!(cfg_abort(200).validate().is_ok());
    assert!(cfg_abort(599).validate().is_ok());
}

#[test]
fn validate_rule1_both_blocks_missing() {
    let err = FaultInjectionConfig::default().validate().unwrap_err();
    assert!(err.contains("delay") && err.contains("abort"), "got: {err}");
}

#[test]
fn validate_rule2_fixed_and_range_both_set() {
    let mut cfg = cfg_delay_fixed(100);
    cfg.delay.as_mut().unwrap().min_ms = Some(10);
    cfg.delay.as_mut().unwrap().max_ms = Some(50);
    let err = cfg.validate().unwrap_err();
    assert!(err.contains("delay.fixedMs"), "got: {err}");
}

#[test]
fn validate_rule2_neither_fixed_nor_range_set() {
    let cfg = FaultInjectionConfig {
        delay: Some(DelayConfig {
            ratio: 1.0,
            fixed_ms: None,
            min_ms: None,
            max_ms: None,
        }),
        abort: None,
    };
    let err = cfg.validate().unwrap_err();
    assert!(err.contains("delay") && err.contains("must"), "got: {err}");
}

#[test]
fn validate_rule3_range_only_min() {
    let cfg = FaultInjectionConfig {
        delay: Some(DelayConfig {
            ratio: 1.0,
            fixed_ms: None,
            min_ms: Some(10),
            max_ms: None,
        }),
        abort: None,
    };
    assert!(cfg.validate().is_err());
}

#[test]
fn validate_rule3_range_min_gt_max() {
    let cfg = FaultInjectionConfig {
        delay: Some(DelayConfig {
            ratio: 1.0,
            fixed_ms: None,
            min_ms: Some(500),
            max_ms: Some(100),
        }),
        abort: None,
    };
    let err = cfg.validate().unwrap_err();
    assert!(err.contains("minMs") && err.contains("maxMs"), "got: {err}");
}

#[test]
fn validate_rule3_range_max_too_large() {
    let cfg = FaultInjectionConfig {
        delay: Some(DelayConfig {
            ratio: 1.0,
            fixed_ms: None,
            min_ms: Some(0),
            max_ms: Some(MAX_DELAY_MS + 1),
        }),
        abort: None,
    };
    assert!(cfg.validate().is_err());
}

#[test]
fn validate_rule4_fixed_too_large() {
    assert!(cfg_delay_fixed(MAX_DELAY_MS + 1).validate().is_err());
}

#[test]
fn validate_rule5_delay_ratio_out_of_range() {
    let mut cfg = cfg_delay_fixed(100);
    cfg.delay.as_mut().unwrap().ratio = 1.5;
    assert!(cfg.validate().is_err());
    cfg.delay.as_mut().unwrap().ratio = -0.1;
    assert!(cfg.validate().is_err());
}

#[test]
fn validate_rule6_abort_ratio_out_of_range() {
    let mut cfg = cfg_abort(500);
    cfg.abort.as_mut().unwrap().ratio = 2.0;
    assert!(cfg.validate().is_err());
}

#[test]
fn validate_rule7_status_out_of_range() {
    for status in [100, 101, 199, 600] {
        assert!(cfg_abort(status).validate().is_err(), "{status}");
    }
}

#[test]
fn abort_status_schema_matches_final_response_range() {
    let schema = serde_json::to_value(schemars::schema_for!(AbortConfig)).unwrap();
    let status = &schema["properties"]["statusCode"];
    assert_eq!(status["minimum"].as_u64(), Some(200));
    assert_eq!(status["maximum"].as_u64(), Some(599));
}

#[test]
fn validate_rule8_bad_header_name() {
    let mut cfg = cfg_abort(500);
    cfg.abort.as_mut().unwrap().headers = Some(vec![FaultHeaderEntry {
        name: "Bad Header\n".to_string(),
        value: "v".to_string(),
    }]);
    let err = cfg.validate().unwrap_err();
    assert!(
        err.to_lowercase().contains("header") || err.contains("name"),
        "got: {err}"
    );
}

#[test]
fn validate_rule9_rejects_content_length_in_headers() {
    let mut cfg = cfg_abort(500);
    cfg.abort.as_mut().unwrap().headers = Some(vec![FaultHeaderEntry {
        name: "Content-Length".to_string(),
        value: "10".to_string(),
    }]);
    let err = cfg.validate().unwrap_err();
    assert_eq!(err, "response header mutation targets a protected protocol field");
}
