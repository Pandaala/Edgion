use std::collections::BTreeMap;

use edgion_resources::resources::common::outbound_tls::{OutboundTlsLocal, OutboundTlsValidation};
use k8s_openapi::ByteString;

use super::*;
use edgion_resources::resources::backend_tls_policy::WellKnownCACertificates;
use edgion_resources::resources::common::TlsConfig;
use k8s_openapi::api::core::v1::Secret;

fn tls_digest(tls: Option<&TlsConfig>, policy: bool) -> [u8; 32] {
    let mut hasher = FingerprintHasher::new();
    if policy {
        hash_tls_policy(&mut hasher, tls);
    } else {
        hash_tls_material(&mut hasher, tls);
    }
    hasher.finish()
}

#[test]
fn shared_tls_policy_distinguishes_validation_variants() {
    let mut tls = TlsConfig {
        enabled: true,
        ..Default::default()
    };
    let base = tls_digest(Some(&tls), true);
    assert_ne!(base, tls_digest(None, true));
    assert_ne!(base, tls_digest(Some(&TlsConfig::default()), true));

    tls.outbound = OutboundTlsLocal {
        verify: Some(false),
        validation: Some(OutboundTlsValidation {
            well_known_ca_certificates: Some(WellKnownCACertificates::System),
            ..Default::default()
        }),
        ..Default::default()
    };
    let system_ca = tls_digest(Some(&tls), true);
    assert_ne!(base, system_ca);
    tls.outbound.validation.as_mut().unwrap().well_known_ca_certificates =
        Some(WellKnownCACertificates::Other("custom".to_string()));
    assert_ne!(system_ca, tls_digest(Some(&tls), true));
}

#[test]
fn shared_tls_material_hashes_only_enabled_secret_data_in_key_order() {
    let mut first = TlsConfig {
        enabled: true,
        ..Default::default()
    };
    first.resolved_ca_certificates = Some(vec![Secret {
        data: Some(BTreeMap::from([
            ("b".to_string(), ByteString(vec![2])),
            ("a".to_string(), ByteString(vec![1])),
        ])),
        ..Default::default()
    }]);
    let mut second = first.clone();
    second.resolved_ca_certificates.as_mut().unwrap()[0].metadata.name = Some("ignored".to_string());
    assert_eq!(tls_digest(Some(&first), false), tls_digest(Some(&second), false));

    second.resolved_ca_certificates.as_mut().unwrap()[0]
        .data
        .as_mut()
        .unwrap()
        .insert("a".to_string(), ByteString(vec![3]));
    assert_ne!(tls_digest(Some(&first), false), tls_digest(Some(&second), false));
    first.enabled = false;
    second.enabled = false;
    assert_eq!(tls_digest(Some(&first), false), tls_digest(Some(&second), false));
    assert_ne!(tls_digest(None, false), tls_digest(Some(&first), false));
}

fn fp(rebuild: u8, secret: u8, rv: Option<&str>) -> RuntimeFingerprint {
    let mut r = [0u8; 32];
    let mut s = [0u8; 32];
    r[0] = rebuild;
    s[0] = secret;
    RuntimeFingerprint::from_parts(r, s, rv.map(str::to_string))
}

#[test]
fn equal_fingerprint_reuses() {
    let live = fp(1, 2, Some("10"));
    let incoming = fp(1, 2, Some("11"));
    assert_eq!(
        plan_apply(
            Some(&live),
            &incoming,
            ConnectionCoverage { live: 3, configured: 8 },
            false
        ),
        ApplyPlan::Reuse
    );
}

#[test]
fn rv_skip_reuses_when_secret_matches_even_if_rebuild_differs() {
    let live = fp(1, 9, Some("10"));
    let incoming = fp(2, 9, Some("10"));
    assert_eq!(
        plan_apply(
            Some(&live),
            &incoming,
            ConnectionCoverage { live: 1, configured: 8 },
            false
        ),
        ApplyPlan::Reuse
    );
}

#[test]
fn rv_equal_secret_change_does_not_skip() {
    let live = fp(1, 9, Some("10"));
    let incoming = fp(1, 8, Some("10"));
    assert_eq!(
        plan_apply(
            Some(&live),
            &incoming,
            ConnectionCoverage { live: 1, configured: 8 },
            false
        ),
        ApplyPlan::Handover
    );
    assert_eq!(
        plan_apply(
            Some(&live),
            &incoming,
            ConnectionCoverage { live: 0, configured: 8 },
            false
        ),
        ApplyPlan::ReplaceNow
    );
}

#[test]
fn live_zero_replace_now_on_rebuild() {
    let live = fp(1, 1, None);
    let incoming = fp(2, 1, None);
    assert_eq!(
        plan_apply(
            Some(&live),
            &incoming,
            ConnectionCoverage { live: 0, configured: 8 },
            false
        ),
        ApplyPlan::ReplaceNow
    );
}

#[test]
fn live_positive_handover_on_rebuild() {
    let live = fp(1, 1, None);
    let incoming = fp(2, 1, None);
    assert_eq!(
        plan_apply(
            Some(&live),
            &incoming,
            ConnectionCoverage { live: 2, configured: 8 },
            false
        ),
        ApplyPlan::Handover
    );
}

#[test]
fn forced_bypasses_reuse() {
    let live = fp(1, 1, Some("10"));
    let incoming = fp(1, 1, Some("10"));
    assert_eq!(
        plan_apply(
            Some(&live),
            &incoming,
            ConnectionCoverage { live: 4, configured: 8 },
            true
        ),
        ApplyPlan::Handover
    );
    assert_eq!(
        plan_apply(
            Some(&live),
            &incoming,
            ConnectionCoverage { live: 0, configured: 0 },
            true
        ),
        ApplyPlan::ReplaceNow
    );
}

#[test]
fn unspecified_fingerprint_does_not_reuse() {
    let live = RuntimeFingerprint::unspecified();
    let incoming = RuntimeFingerprint::unspecified();
    assert_eq!(
        plan_apply(
            Some(&live),
            &incoming,
            ConnectionCoverage { live: 3, configured: 8 },
            false
        ),
        ApplyPlan::Handover
    );

    let live_rv = RuntimeFingerprint::from_parts([0; 32], [0; 32], Some("1".to_string()));
    let incoming_rv = RuntimeFingerprint::from_parts([0; 32], [0; 32], Some("1".to_string()));
    assert!(live_rv.is_unspecified());
    assert_eq!(
        plan_apply(
            Some(&live_rv),
            &incoming_rv,
            ConnectionCoverage { live: 1, configured: 8 },
            false
        ),
        ApplyPlan::Handover
    );
}

#[test]
fn hasher_length_prefix_avoids_concat_collision() {
    let mut a = FingerprintHasher::new();
    a.bytes(b"ab");
    a.bytes(b"c");
    let mut b = FingerprintHasher::new();
    b.bytes(b"a");
    b.bytes(b"bc");
    assert_ne!(a.finish(), b.finish());
}

#[test]
fn debug_redacts_hashes() {
    let rendered = format!("{:?}", fp(7, 8, Some("1")));
    assert!(rendered.contains("<redacted>"));
    assert!(!rendered.contains("07"));
}

#[test]
fn connect_backoff_doubles_and_clamps() {
    let next = next_connect_backoff(CONNECT_BACKOFF_START, CONNECT_BACKOFF_MAX);
    assert_eq!(next, Duration::from_secs(4));
    assert_eq!(
        next_connect_backoff(Duration::from_secs(16), CONNECT_BACKOFF_MAX),
        Duration::from_secs(30)
    );
    assert_eq!(
        next_connect_backoff(CONNECT_BACKOFF_MAX, CONNECT_BACKOFF_MAX),
        CONNECT_BACKOFF_MAX
    );
}
