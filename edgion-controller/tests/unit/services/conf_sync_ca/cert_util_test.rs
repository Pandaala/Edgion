use super::*;
use edgion_common::conf_sync::cert_duration::CertDuration;
use edgion_common::conf_sync::config::{ConfSyncRotationConfig, RotationPolicy};
use time::macros::datetime;

fn scheduled_cfg(guard_h: u64) -> ConfSyncRotationConfig {
    ConfSyncRotationConfig {
        policy: RotationPolicy::Scheduled,
        schedule: Some("0 3 * * 0".into()), // Sunday 03:00 weekly
        rotate_if_remaining_below: CertDuration::from_hours(guard_h),
        ..Default::default()
    }
}

// (a) weekly cron x guard 336h x ttl 365d: exactly one in-window rotation;
// an aged leaf crossing the guard mid-week does NOT rotate until the firing.
#[test]
fn scheduled_rotates_only_at_firing() {
    let cfg = scheduled_cfg(336);
    let nb = datetime!(2025-01-05 03:00 UTC); // a Sunday, rotated exactly at a firing
                                              // Leaf signed AT the firing, notBefore backdated:
    let leaf = (nb - BACKDATE_SLACK, nb - BACKDATE_SLACK + time::Duration::days(365));
    let fresh_server = leaf; // server leaf same window; irrelevant here

    // Mid-week guard crossing: remaining hits 336h on a Wednesday. Not due.
    let crossing = leaf.1 - time::Duration::hours(336) + time::Duration::minutes(1);
    let last_sunday = datetime!(2025-12-28 03:00 UTC); // a firing after `crossing` (stale last_firing input)
    let d = rotation_due(
        &cfg,
        &PolicyInputs {
            client_leaf: leaf,
            server_leaf: fresh_server,
            last_firing: Some(last_sunday),
        },
        crossing,
    );
    // guard at FIRING time: leaf.1 - last_sunday > 336h -> not due
    assert!(
        matches!(d, RotationDecision::NotDue),
        "must wait for the next firing, got {d:?}"
    );

    // Next firing: now due (remaining at firing <= 336h) and not yet acted on.
    let next_firing = datetime!(2026-01-04 03:00 UTC);
    let d = rotation_due(
        &cfg,
        &PolicyInputs {
            client_leaf: leaf,
            server_leaf: fresh_server,
            last_firing: Some(next_firing),
        },
        next_firing + time::Duration::minutes(1),
    );
    assert!(matches!(d, RotationDecision::Due(_)));
}

// (e) a just-rotated leaf never re-triggers its own firing (BACKDATE_SLACK arithmetic)
#[test]
fn fresh_leaf_does_not_retrigger_its_firing() {
    let cfg = scheduled_cfg(336);
    let firing = datetime!(2026-01-04 03:00 UTC);
    // Rotation completed 40s after the firing; signer backdates notBefore by 5min.
    let signed_at = firing + time::Duration::seconds(40);
    let leaf = (
        signed_at - BACKDATE_SLACK,
        signed_at - BACKDATE_SLACK + time::Duration::days(365),
    );
    for tick in [1i64, 60, 300, 3600] {
        let d = rotation_due(
            &cfg,
            &PolicyInputs {
                client_leaf: leaf,
                server_leaf: leaf,
                last_firing: Some(firing),
            },
            firing + time::Duration::seconds(40 + tick),
        );
        assert!(matches!(d, RotationDecision::NotDue), "tick +{tick}s re-triggered");
    }
}

// (b) failover straddling a due firing -> catch-up rotates; straddling a
// guard-skipped firing -> still skipped.
#[test]
fn failover_catchup_semantics() {
    let cfg = scheduled_cfg(336);
    let firing = datetime!(2026-01-04 03:00 UTC);
    let takeover = firing + time::Duration::hours(6);
    // due at the firing (remaining 200h)
    let due_leaf = (
        firing - time::Duration::days(365) + time::Duration::hours(200),
        firing + time::Duration::hours(200),
    );
    let d = rotation_due(
        &cfg,
        &PolicyInputs {
            client_leaf: due_leaf,
            server_leaf: due_leaf,
            last_firing: Some(firing),
        },
        takeover,
    );
    assert!(matches!(d, RotationDecision::Due(_)), "missed due window must catch up");
    // NOT due at the firing (remaining 2000h) -> stays skipped after takeover
    let fresh_leaf = (
        firing - time::Duration::days(365) + time::Duration::hours(2000),
        firing + time::Duration::hours(2000),
    );
    let d = rotation_due(
        &cfg,
        &PolicyInputs {
            client_leaf: fresh_leaf,
            server_leaf: fresh_leaf,
            last_firing: Some(firing),
        },
        takeover,
    );
    assert!(matches!(d, RotationDecision::NotDue));
}

// (c) monthly cron worst alignment: emergency net fires at 24h, never expiry.
#[test]
fn emergency_net_covers_skipped_windows() {
    let mut cfg = scheduled_cfg(336);
    cfg.schedule = Some("0 3 1 * *".into()); // monthly
    let na = datetime!(2026-06-15 12:00 UTC);
    let leaf = (na - time::Duration::days(365), na);
    // 23h before expiry, last firing skipped long ago
    let now = na - time::Duration::hours(23);
    let last = datetime!(2026-06-01 03:00 UTC);
    let d = rotation_due(
        &cfg,
        &PolicyInputs {
            client_leaf: leaf,
            server_leaf: leaf,
            last_firing: Some(last),
        },
        now,
    );
    assert!(matches!(d, RotationDecision::EmergencyDue(_)));
}

// Emergency net checks BOTH windows (server leaf expiring alone triggers it).
#[test]
fn emergency_net_checks_server_leaf_too() {
    let cfg = ConfSyncRotationConfig {
        policy: RotationPolicy::Manual,
        ..Default::default()
    };
    let now = datetime!(2026-06-15 12:00 UTC);
    let fresh = (now - time::Duration::days(1), now + time::Duration::days(364));
    let dying = (now - time::Duration::days(364), now + time::Duration::hours(3));
    let d = rotation_due(
        &cfg,
        &PolicyInputs {
            client_leaf: fresh,
            server_leaf: dying,
            last_firing: None,
        },
        now,
    );
    assert!(matches!(d, RotationDecision::EmergencyDue(_)));
}

#[test]
fn lifetime_policy_uses_client_leaf_only() {
    let cfg = ConfSyncRotationConfig::default(); // lifetime 0.5, ttl 365d
    let now = datetime!(2026-06-15 12:00 UTC);
    let old = (now - time::Duration::days(200), now + time::Duration::days(165)); // past half-life
    let fresh = (now - time::Duration::days(1), now + time::Duration::days(364));
    // old CLIENT leaf -> due
    let d = rotation_due(
        &cfg,
        &PolicyInputs {
            client_leaf: old,
            server_leaf: fresh,
            last_firing: None,
        },
        now,
    );
    assert!(matches!(d, RotationDecision::Due(_)));
    // old SERVER leaf only -> NOT a policy trigger (spec §4.4)
    let d = rotation_due(
        &cfg,
        &PolicyInputs {
            client_leaf: fresh,
            server_leaf: old,
            last_firing: None,
        },
        now,
    );
    assert!(matches!(d, RotationDecision::NotDue));
}

#[test]
fn ca_renewal_threshold_is_self_scaling() {
    let now = datetime!(2026-06-15 12:00 UTC);
    // 10y root, 40d remaining -> not due (threshold min(30d, 1y) = 30d)
    assert!(!ca_renewal_due(
        (now - time::Duration::days(3610), now + time::Duration::days(40)),
        now
    ));
    // 10y root, 20d remaining -> due
    assert!(ca_renewal_due(
        (now - time::Duration::days(3630), now + time::Duration::days(20)),
        now
    ));
    // 10d root (test cluster), just renewed (10d remaining): threshold = 1d -> NOT due (anti-storm)
    assert!(!ca_renewal_due((now, now + time::Duration::days(10)), now));
    // 10d root, 12h remaining -> due
    assert!(ca_renewal_due(
        (
            now - time::Duration::days(10) + time::Duration::hours(12),
            now + time::Duration::hours(12)
        ),
        now
    ));
}

// Cron helpers (needs croner) — (d) degenerate frequencies + feasibility.
#[test]
fn cron_helpers() {
    // last firing of daily 03:00 UTC as of 04:00 is today's 03:00
    let now = datetime!(2026-06-15 04:00 UTC);
    let f = last_firing("0 3 * * *", "UTC", now).unwrap().unwrap();
    assert_eq!(f, datetime!(2026-06-15 03:00 UTC));
    // max gap of a weekly cron ~= 7d
    let gap = max_cron_gap("0 3 * * 0", "UTC").unwrap().unwrap();
    assert_eq!(gap.as_secs(), 7 * 24 * 3600);
    // per-minute cron: gap 60s
    let gap = max_cron_gap("* * * * *", "UTC").unwrap().unwrap();
    assert_eq!(gap.as_secs(), 60);
    // Feb 30 never fires
    assert!(max_cron_gap("0 3 30 2 *", "UTC").unwrap().is_none());
    // bad tz
    assert!(last_firing("0 3 * * *", "Not/AZone", now).is_err());
}

#[test]
fn validate_schedule_controller_warns_and_rejects() {
    // weekly cron with guard 168h (= exactly one gap) -> suspect-combination warning
    // PLUS the expiry-watch WARN-band cross-warn (168h < 720h WARN_THRESHOLD).
    let mut cfg = scheduled_cfg(168);
    cfg.leaf_ttl = CertDuration::from_hours(365 * 24);
    let warnings = validate_schedule_controller(&cfg).unwrap();
    assert_eq!(warnings.len(), 2, "expected both warnings, got: {warnings:?}");
    // guard 336h (= 2 gaps, clears the suspect-combination check) still trips the
    // expiry-watch cross-warn (336h < 720h WARN_THRESHOLD) — this is the DEFAULT
    // guard, and the cross-warn firing on it is intended (see doc comment).
    let cfg = scheduled_cfg(336);
    let warnings = validate_schedule_controller(&cfg).unwrap();
    assert_eq!(
        warnings.len(),
        1,
        "expected only the WARN-band cross-warn, got: {warnings:?}"
    );
    // guard 720h (= WARN_THRESHOLD) -> fully clean: clears both checks.
    let cfg = scheduled_cfg(720);
    assert!(validate_schedule_controller(&cfg).unwrap().is_empty());
    // never-firing cron -> hard error
    let mut cfg = scheduled_cfg(336);
    cfg.schedule = Some("0 3 30 2 *".into());
    assert!(validate_schedule_controller(&cfg).is_err());
}

// -- Task C: expiry-watch WARN-band cross-warn table tests --

fn lifetime_cfg(lifetime_ratio: f64, leaf_ttl_hours: u64) -> ConfSyncRotationConfig {
    ConfSyncRotationConfig {
        policy: RotationPolicy::Lifetime,
        lifetime_ratio,
        leaf_ttl: CertDuration::from_hours(leaf_ttl_hours),
        ca_ttl: CertDuration::from_hours(leaf_ttl_hours * 20),
        emergency_rotate_below: CertDuration::from_hours(1),
        ..Default::default()
    }
}

#[test]
fn lifetime_policy_half_life_on_a_year_leaf_does_not_warn() {
    // 0.5 x 365d leaves 182.5d remaining at rotation, well outside the 30d WARN band.
    let cfg = lifetime_cfg(0.5, 365 * 24);
    assert!(validate_schedule_controller(&cfg).unwrap().is_empty());
}

#[test]
fn lifetime_policy_late_rotation_on_a_short_leaf_warns() {
    // 0.9 x 90d leaves 9d remaining at rotation -> inside the 30d WARN band.
    let cfg = lifetime_cfg(0.9, 90 * 24);
    let warnings = validate_schedule_controller(&cfg).unwrap();
    assert_eq!(
        warnings.len(),
        1,
        "expected the WARN-band cross-warn, got: {warnings:?}"
    );
}

#[test]
fn scheduled_default_guard_336h_warns() {
    let cfg = scheduled_cfg(336);
    let warnings = validate_schedule_controller(&cfg).unwrap();
    assert_eq!(
        warnings.len(),
        1,
        "expected the WARN-band cross-warn, got: {warnings:?}"
    );
}

#[test]
fn scheduled_guard_720h_does_not_warn() {
    let cfg = scheduled_cfg(720);
    assert!(validate_schedule_controller(&cfg).unwrap().is_empty());
}

// -- chains_to (spec: emergency re-key / BYO-root swap self-heal) --

use crate::services::conf_sync_ca::Ca;

#[test]
fn chains_to_true_for_leaf_signed_by_the_ca() {
    let ca = Ca::generate_root(time::Duration::days(3650)).unwrap();
    let leaf = ca.sign_client(time::Duration::days(30)).unwrap();
    assert!(chains_to(&leaf.cert_pem, ca.ca_cert_pem()));
}

#[test]
fn chains_to_false_for_a_different_root() {
    let ca_a = Ca::generate_root(time::Duration::days(3650)).unwrap();
    let ca_b = Ca::generate_root(time::Duration::days(3650)).unwrap();
    let leaf = ca_a.sign_client(time::Duration::days(30)).unwrap();
    assert!(!chains_to(&leaf.cert_pem, ca_b.ca_cert_pem()));
}

#[test]
fn chains_to_false_for_garbage_inputs() {
    assert!(!chains_to("not a pem", "also not a pem"));
    let ca = Ca::generate_root(time::Duration::days(3650)).unwrap();
    let leaf = ca.sign_client(time::Duration::days(30)).unwrap();
    assert!(!chains_to(&leaf.cert_pem, "not a pem"));
    assert!(!chains_to("not a pem", ca.ca_cert_pem()));
}

// Same-key property: a leaf signed by a same-key-RENEWED root still chains to
// the OLD root cert — proves normal renewal never trips the out-of-sync check.
#[test]
fn chains_to_true_for_leaf_from_a_same_key_renewed_root_against_the_old_root_cert() {
    let ca = Ca::generate_root(time::Duration::days(3650)).unwrap();
    let renewed = ca.renew_root_same_key(time::Duration::days(3650)).unwrap();
    let leaf = renewed.sign_client(time::Duration::days(30)).unwrap();
    assert!(chains_to(&leaf.cert_pem, ca.ca_cert_pem()));
}
