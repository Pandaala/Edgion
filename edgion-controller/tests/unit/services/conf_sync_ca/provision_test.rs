use super::*;

#[test]
fn server_sans_include_leader_service_fqdn_and_loopback() {
    let sans = server_sans_for("edgion-system");
    assert!(sans.contains(&"127.0.0.1".to_string()));
    // The leader-Service FQDN is the host the gateway verifies against — it MUST
    // be present or every mTLS handshake fails hostname verification.
    assert!(sans.contains(&"edgion-controller-leader.edgion-system.svc.cluster.local".to_string()));
    // The direct Service name is kept too.
    assert!(sans.contains(&"edgion-controller.edgion-system.svc.cluster.local".to_string()));
}

#[test]
fn leaf_needs_rewrite_decision_table() {
    use crate::services::conf_sync_ca::cert_util::BACKDATE_SLACK;
    use crate::services::conf_sync_ca::Ca;
    let _ = BACKDATE_SLACK; // silence if unused
    let ca = Ca::generate_root(time::Duration::days(365)).unwrap();
    let leaf = ca.sign_client(time::Duration::days(30)).unwrap();
    let now = time::OffsetDateTime::now_utc();
    let emergency = time::Duration::hours(24);
    // healthy existing leaf -> adopt
    assert!(!leaf_needs_rewrite(
        Some((&leaf.cert_pem, leaf.key_pem.as_str())),
        ca.ca_cert_pem(),
        emergency,
        now
    ));
    // missing -> rewrite
    assert!(leaf_needs_rewrite(None, ca.ca_cert_pem(), emergency, now));
    // garbage -> rewrite
    assert!(leaf_needs_rewrite(
        Some(("bad", "bad")),
        ca.ca_cert_pem(),
        emergency,
        now
    ));
    // wrong CA (stale BYO-root leaf) -> rewrite
    let other = Ca::generate_root(time::Duration::days(365)).unwrap();
    assert!(leaf_needs_rewrite(
        Some((&leaf.cert_pem, leaf.key_pem.as_str())),
        other.ca_cert_pem(),
        emergency,
        now
    ));
    // near expiry -> rewrite (1-day leaf, 25h emergency threshold: the leaf's
    // entire remaining validity is inside the emergency window)
    let dying = ca.sign_client(time::Duration::days(1)).unwrap();
    assert!(leaf_needs_rewrite(
        Some((&dying.cert_pem, dying.key_pem.as_str())),
        ca.ca_cert_pem(),
        time::Duration::hours(25),
        now
    ));
}
