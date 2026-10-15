use super::*;

// ── Helper ───────────────────────────────────────────────────────────────

fn make_tls(name: Option<&str>, cert: &str) -> ConfSyncTlsConfig {
    ConfSyncTlsConfig {
        name: name.map(|s| s.to_string()),
        cert: cert.to_string(),
        key: "k".to_string(),
        ca: "ca".to_string(),
        skip_tls: false,
    }
}

// ── Group A: get_capacity (3 → 1 table-driven) ──────────────────────────

#[test]
fn test_get_capacity_table_driven() {
    struct Case {
        name: &'static str,
        default_capacity: u32,
        override_entry: Option<(&'static str, u32)>, // (kind, cap) to insert
        query_kind: &'static str,
        expected: u32,
    }

    let cases = [
        Case {
            name: "override wins for known kind",
            default_capacity: 200,
            override_entry: Some(("HTTPRoute", 999)),
            query_kind: "HTTPRoute",
            expected: 999,
        },
        Case {
            name: "falls back to default when no overrides",
            default_capacity: 200,
            override_entry: None,
            query_kind: "GatewayClass",
            expected: 200,
        },
        Case {
            name: "unknown kind falls back to global default",
            default_capacity: 200,
            override_entry: None,
            query_kind: "SomeUnknownKind",
            expected: 200,
        },
    ];

    for case in &cases {
        let capacity_overrides = case.override_entry.map(|(kind, cap)| {
            let mut m = HashMap::new();
            m.insert(kind.to_string(), cap);
            m
        });
        let config = ConfSyncConfig {
            default_capacity: case.default_capacity,
            capacity_overrides,
            no_sync_kinds: None,
        };
        assert_eq!(
            config.get_capacity(case.query_kind),
            case.expected,
            "case: {}",
            case.name
        );
    }
}

#[test]
fn test_conf_sync_default() {
    let config = ConfSyncConfig::default();
    assert_eq!(config.default_capacity, 200);
    assert!(config.capacity_overrides.is_none());
}

#[test]
fn auto_ca_defaults_false_and_parses() {
    assert!(!ConfSyncSecurityConfig::default().auto_ca);
    let parsed: ConfSyncSecurityConfig = serde_json::from_str(r#"{"auto_ca": true}"#).unwrap();
    assert!(parsed.auto_ca);
    // Omitting the field keeps the backward-compatible default.
    let omitted: ConfSyncSecurityConfig = serde_json::from_str("{}").unwrap();
    assert!(!omitted.auto_ca);
}

// ── Group C: resolve_active_tls (5 → 1 table-driven) ───────────────────

#[test]
fn test_resolve_active_tls_scenarios() {
    struct Case {
        name: &'static str,
        cfg: ConfSyncSecurityConfig,
        expected_cert: Option<&'static str>,
    }

    let cases = [
        Case {
            name: "unnamed single cert entry with no active selector",
            cfg: ConfSyncSecurityConfig {
                certs: vec![make_tls(None, "unnamed.crt")],
                ..Default::default()
            },
            expected_cert: Some("unnamed.crt"),
        },
        Case {
            name: "single cert entry with no active selector",
            cfg: ConfSyncSecurityConfig {
                certs: vec![make_tls(Some("primary"), "primary.crt")],
                ..Default::default()
            },
            expected_cert: Some("primary.crt"),
        },
        Case {
            name: "multi certs with matching active selector",
            cfg: ConfSyncSecurityConfig {
                active: Some("new".to_string()),
                certs: vec![make_tls(Some("old"), "old.crt"), make_tls(Some("new"), "new.crt")],
                ..Default::default()
            },
            expected_cert: Some("new.crt"),
        },
        Case {
            name: "multi certs without active selector returns None",
            cfg: ConfSyncSecurityConfig {
                certs: vec![make_tls(Some("a"), "a.crt"), make_tls(Some("b"), "b.crt")],
                ..Default::default()
            },
            expected_cert: None,
        },
        Case {
            name: "empty config returns None",
            cfg: ConfSyncSecurityConfig::default(),
            expected_cert: None,
        },
    ];

    for case in &cases {
        assert_eq!(
            case.cfg.resolve_active_tls().map(|t| t.cert.as_str()),
            case.expected_cert,
            "case: {}",
            case.name
        );
    }
}

// ── Group D: validate_active_tls (5 → 1 table-driven) ──────────────────

#[test]
fn test_validate_active_tls_scenarios() {
    enum Expected {
        Ok,
        Err(&'static str),
    }

    struct Case {
        name: &'static str,
        cfg: ConfSyncSecurityConfig,
        expected: Expected,
    }

    let cases = [
        Case {
            name: "empty config is valid",
            cfg: ConfSyncSecurityConfig::default(),
            expected: Expected::Ok,
        },
        Case {
            name: "single entry with skip_tls is valid",
            cfg: ConfSyncSecurityConfig {
                certs: vec![ConfSyncTlsConfig {
                    name: Some("p".to_string()),
                    cert: String::new(),
                    key: String::new(),
                    ca: String::new(),
                    skip_tls: true,
                }],
                ..Default::default()
            },
            expected: Expected::Ok,
        },
        Case {
            name: "duplicate cert names errors",
            cfg: ConfSyncSecurityConfig {
                active: Some("dup".to_string()),
                certs: vec![make_tls(Some("dup"), "a.crt"), make_tls(Some("dup"), "b.crt")],
                ..Default::default()
            },
            expected: Expected::Err(""),
        },
        Case {
            name: "multi certs without active selector errors",
            cfg: ConfSyncSecurityConfig {
                certs: vec![make_tls(Some("a"), "a.crt"), make_tls(Some("b"), "b.crt")],
                ..Default::default()
            },
            expected: Expected::Err("no 'active' selector"),
        },
        Case {
            name: "active selector not found in certs errors",
            cfg: ConfSyncSecurityConfig {
                active: Some("missing".to_string()),
                certs: vec![make_tls(Some("primary"), "primary.crt")],
                ..Default::default()
            },
            expected: Expected::Err("does not match any cert name"),
        },
    ];

    for case in &cases {
        let result = case.cfg.validate();
        match &case.expected {
            Expected::Ok => {
                assert!(result.is_ok(), "case '{}' expected Ok, got {:?}", case.name, result);
            }
            Expected::Err(substr) => {
                assert!(result.is_err(), "case '{}' expected Err, got Ok", case.name);
                if !substr.is_empty() {
                    let msg = format!("{:?}", result.unwrap_err());
                    assert!(
                        msg.contains(substr),
                        "case '{}': error message {:?} does not contain {:?}",
                        case.name,
                        msg,
                        substr
                    );
                }
            }
        }
    }
}

// ── Group G: resolve_mtls_or_refuse ─────────────────────────────────────

#[test]
fn resolve_mtls_or_refuse_no_tls() {
    let sec = ConfSyncSecurityConfig::default();
    assert_eq!(sec.resolve_mtls_or_refuse().unwrap_err(), MtlsRefusal::NoTls);
}

#[test]
fn resolve_mtls_or_refuse_skip_tls() {
    let sec = ConfSyncSecurityConfig {
        certs: vec![ConfSyncTlsConfig {
            name: None,
            cert: "c".into(),
            key: "k".into(),
            ca: "a".into(),
            skip_tls: true,
        }],
        ..Default::default()
    };
    assert_eq!(sec.resolve_mtls_or_refuse().unwrap_err(), MtlsRefusal::SkipTls);
}

#[test]
fn resolve_mtls_or_refuse_ok_for_mtls() {
    let sec = ConfSyncSecurityConfig {
        certs: vec![ConfSyncTlsConfig {
            name: None,
            cert: "c".into(),
            key: "k".into(),
            ca: "a".into(),
            skip_tls: false,
        }],
        ..Default::default()
    };
    assert!(sec.resolve_mtls_or_refuse().is_ok());
}

#[test]
fn is_plaintext_omitted_scenarios() {
    // Truly empty section → omitted.
    assert!(ConfSyncSecurityConfig::default().is_plaintext_omitted());

    // An unnamed single cert is also an explicit selection.
    let with_unnamed_cert = ConfSyncSecurityConfig {
        certs: vec![make_tls(None, "c.crt")],
        ..Default::default()
    };
    assert!(!with_unnamed_cert.is_plaintext_omitted());

    // certs present → not omitted.
    let with_certs = ConfSyncSecurityConfig {
        certs: vec![make_tls(Some("primary"), "p.crt")],
        ..Default::default()
    };
    assert!(!with_certs.is_plaintext_omitted());

    // active set but nothing else → NOT omitted (it is a dangling misconfig).
    let dangling = ConfSyncSecurityConfig {
        active: Some("ghost".to_string()),
        ..Default::default()
    };
    assert!(!dangling.is_plaintext_omitted());
}

#[test]
fn ensure_transport_configured_rejects_omitted_section() {
    // Fail-close (2026-07-14): an omitted section must refuse to start.
    let err = ConfSyncSecurityConfig::default()
        .ensure_transport_configured()
        .expect_err("omitted conf_sync_security must be rejected");
    let msg = format!("{err:?}");
    // The message must name all three ways forward.
    for needle in ["auto_ca", "certs", "skip_tls", "Secrets"] {
        assert!(msg.contains(needle), "error must mention {needle:?}, got: {msg}");
    }
}

#[test]
fn ensure_transport_configured_accepts_skip_tls_and_file_mode() {
    // skip_tls stays the explicit plaintext escape hatch.
    let skip = ConfSyncSecurityConfig {
        certs: vec![ConfSyncTlsConfig {
            name: None,
            cert: String::new(),
            key: String::new(),
            ca: String::new(),
            skip_tls: true,
        }],
        ..Default::default()
    };
    assert!(skip.ensure_transport_configured().is_ok());

    // File-mode mTLS is unaffected.
    let file_mode = ConfSyncSecurityConfig {
        certs: vec![make_tls(Some("primary"), "p.crt")],
        ..Default::default()
    };
    assert!(file_mode.ensure_transport_configured().is_ok());
}

#[test]
fn ensure_transport_configured_accepts_bare_auto_ca() {
    // The shipped Kubernetes controller configmap is exactly `{auto_ca: true}` with
    // no cert paths (the serving identity is minted in memory). That is a complete
    // transport choice, NOT the omitted state — it must keep starting.
    let k8s_default: ConfSyncSecurityConfig = serde_json::from_str(r#"{"auto_ca": true}"#).unwrap();
    assert!(k8s_default.is_plaintext_omitted(), "no cert paths are configured");
    assert!(
        k8s_default.ensure_transport_configured().is_ok(),
        "auto_ca is a configured transport; the K8s default must not fail-close"
    );
}

#[test]
fn skip_tls_escape_hatch_parses_without_cert_paths() {
    // The documented migration must be writable as a bare `skip_tls: true` — no
    // dummy cert/key/ca paths. It stays valid and is NOT the omitted state.
    let parsed: ConfSyncSecurityConfig = serde_json::from_str(r#"{"certs": [{"skip_tls": true}]}"#).unwrap();
    assert!(!parsed.is_plaintext_omitted());
    assert!(parsed.validate().is_ok());
    assert!(parsed.ensure_transport_configured().is_ok());
    assert!(parsed.resolve_active_tls().unwrap().skip_tls);
}

#[test]
fn validate_still_rejects_empty_paths_without_skip_tls() {
    // Making cert/key/ca `#[serde(default)]` must not let a real mTLS set through
    // with missing paths.
    let parsed: ConfSyncSecurityConfig = serde_json::from_str(r#"{"certs": [{"cert": "c.crt"}]}"#).unwrap();
    let err = parsed.validate().expect_err("missing key/ca must still be rejected");
    assert!(format!("{err:?}").contains("must not be empty"));
}

#[test]
fn validate_stays_lenient_on_empty_section() {
    // The fail-close lives in `ensure_transport_configured`, NOT in `validate`:
    // `validate` is also used by consumers that merely parse the block (the
    // gateway parsing the controller-only `rotation` sub-block) and must keep
    // accepting an entirely-empty section.
    assert!(ConfSyncSecurityConfig::default().validate().is_ok());
    let rotation_only: ConfSyncSecurityConfig =
        serde_json::from_str(r#"{"rotation": {"policy": "lifetime"}}"#).unwrap();
    assert!(rotation_only.validate().is_ok());
}

#[test]
fn validate_rejects_dangling_active_with_no_certs() {
    let cfg = ConfSyncSecurityConfig {
        active: Some("ghost".to_string()),
        ..Default::default()
    };
    let err = cfg.validate().expect_err("dangling active must be rejected");
    assert!(
        format!("{err:?}").contains("active"),
        "error should mention 'active', got: {err:?}"
    );
}

// ── Group H: rotation configuration ──────────────────────────────────────

#[test]
fn rotation_defaults_are_spec_values() {
    let r = ConfSyncRotationConfig::default();
    assert!(matches!(r.policy, RotationPolicy::Lifetime));
    assert_eq!(r.lifetime_ratio, 0.5);
    assert_eq!(r.schedule, None);
    assert_eq!(r.schedule_timezone, "UTC");
    assert_eq!(r.rotate_if_remaining_below, CertDuration::from_hours(336));
    assert_eq!(r.emergency_rotate_below, CertDuration::from_hours(24));
    assert_eq!(r.leaf_ttl, CertDuration::from_hours(365 * 24));
    assert_eq!(r.ca_ttl, CertDuration::from_hours(3650 * 24));
    assert!(r.validate_numeric().is_ok());
}

#[test]
fn rotation_parses_from_security_block() {
    let parsed: ConfSyncSecurityConfig = serde_json::from_str(
        r#"{"auto_ca": true, "rotation": {"policy": "scheduled", "schedule": "0 3 * * *", "leaf_ttl": "30d"}}"#,
    )
    .unwrap();
    let r = parsed.rotation.unwrap();
    assert!(matches!(r.policy, RotationPolicy::Scheduled));
    assert_eq!(r.leaf_ttl, CertDuration::parse("30d").unwrap());
}

#[test]
fn rotation_numeric_validation_rejects_bad_combos() {
    let ok = ConfSyncRotationConfig::default();
    // churn hole: guard > 0.5 * leaf_ttl
    let mut bad = ok.clone();
    bad.policy = RotationPolicy::Scheduled;
    bad.schedule = Some("0 3 * * *".into());
    bad.leaf_ttl = CertDuration::parse("169h").unwrap();
    bad.rotate_if_remaining_below = CertDuration::parse("168h").unwrap();
    bad.emergency_rotate_below = CertDuration::parse("1h").unwrap();
    assert!(bad.validate_numeric().is_err());
    // lifetime: emergency net would always preempt the lifetime trigger
    let mut bad = ok.clone();
    bad.lifetime_ratio = 0.999;
    assert!(bad.validate_numeric().is_err());
    // ratio bounds
    for ratio in [0.0, 1.0, -0.1, 1.5] {
        let mut bad = ok.clone();
        bad.lifetime_ratio = ratio;
        assert!(bad.validate_numeric().is_err());
    }
    // scheduled requires a schedule
    let mut bad = ok.clone();
    bad.policy = RotationPolicy::Scheduled;
    bad.schedule = None;
    assert!(bad.validate_numeric().is_err());
    // ttl ordering
    let mut bad = ok.clone();
    bad.leaf_ttl = CertDuration::parse("11y").unwrap(); // > default ca_ttl (3650d)
    assert!(bad.validate_numeric().is_err());
}

#[test]
fn retired_tls_material_cannot_satisfy_a_dangling_selector() {
    // ARCH-074: this used to validate and silently select the unnamed legacy set.
    let parsed: ConfSyncSecurityConfig = serde_json::from_str(
        r#"{"active":"ghost","certs":[],"tls":{"cert":"old.crt","key":"old.key","ca":"old-ca.crt"}}"#,
    )
    .unwrap();
    assert!(parsed.validate().unwrap_err().to_string().contains("no certs"));
    assert!(parsed.resolve_active_tls().is_none());
}

#[test]
fn retired_tls_only_is_not_a_transport_choice() {
    for old in [
        r#"{"tls":{"cert":"old.crt","key":"old.key","ca":"old-ca.crt"}}"#,
        r#"{"tls":{"skip_tls":true}}"#,
    ] {
        let parsed: ConfSyncSecurityConfig = serde_json::from_str(old).unwrap();
        assert!(parsed.validate().is_ok());
        assert!(parsed.is_plaintext_omitted());
        assert!(parsed.resolve_active_tls().is_none());
        assert!(parsed.ensure_transport_configured().is_err());
    }
}

#[test]
fn canonical_selection_ignores_retired_material_and_serializes_only_certs() {
    let parsed: ConfSyncSecurityConfig = serde_json::from_str(
        r#"{"active":"primary","certs":[{"name":"primary","cert":"new.crt","key":"new.key","ca":"new-ca.crt"}],"tls":{"skip_tls":true}}"#,
    )
    .unwrap();
    parsed.validate().unwrap();
    parsed.ensure_transport_configured().unwrap();
    let selected = parsed.resolve_mtls_or_refuse().unwrap();
    assert_eq!(selected.cert, "new.crt");
    assert!(!selected.skip_tls);
    let wire = serde_json::to_value(parsed).unwrap();
    assert!(wire.get("tls").is_none());
    assert_eq!(wire["certs"][0]["cert"], "new.crt");
}

#[test]
fn active_cert_rejects_each_empty_path_with_canonical_diagnostic() {
    for field in ["cert", "key", "ca"] {
        let mut cert = make_tls(Some("primary"), "primary.crt");
        match field {
            "cert" => cert.cert = " ".into(),
            "key" => cert.key = " ".into(),
            "ca" => cert.ca = " ".into(),
            _ => unreachable!(),
        }
        let security = ConfSyncSecurityConfig {
            certs: vec![cert],
            ..Default::default()
        };
        assert_eq!(
            security.validate().unwrap_err().to_string(),
            format!("conf_sync_security.certs[].{field} must not be empty")
        );
    }
}
