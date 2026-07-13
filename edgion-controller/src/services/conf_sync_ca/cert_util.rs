//! Pure certificate-rotation math (spec §6 Task 1). No I/O, no clocks — `now`
//! and `last_firing` are always inputs, so every decision is table-testable.

use chrono::{DateTime, TimeZone};
use edgion_common::conf_sync::cert_expiry_watch::WARN_THRESHOLD;
use edgion_common::conf_sync::config::{ConfSyncRotationConfig, RotationPolicy};
use time::OffsetDateTime;

/// Leaf notBefore backdate against clock skew (spec §8 F7). The ONE shared
/// constant: `ca.rs` signing (Task 9) and the scheduled-policy comparator below
/// must both use it or a just-rotated leaf re-flags its own firing.
pub const BACKDATE_SLACK: time::Duration = time::Duration::minutes(5);

#[derive(Debug)]
pub enum RotationDecision {
    NotDue,
    Due(&'static str),
    EmergencyDue(&'static str),
}

pub struct PolicyInputs {
    pub client_leaf: (OffsetDateTime, OffsetDateTime),
    pub server_leaf: (OffsetDateTime, OffsetDateTime),
    /// Most recent cron firing at or before `now`; None for non-scheduled policies
    /// or when the schedule has not fired yet.
    pub last_firing: Option<OffsetDateTime>,
}

pub fn parse_cert_window(cert_pem: &str) -> anyhow::Result<(OffsetDateTime, OffsetDateTime)> {
    let (_, doc) =
        x509_parser::pem::parse_x509_pem(cert_pem.as_bytes()).map_err(|e| anyhow::anyhow!("cert PEM parse: {e}"))?;
    let cert = doc.parse_x509().map_err(|e| anyhow::anyhow!("cert DER parse: {e}"))?;
    let v = cert.validity();
    Ok((v.not_before.to_datetime(), v.not_after.to_datetime()))
}

/// True if `leaf_pem` chains to `ca_pem` (issuer DN matches AND the CA key
/// verifies the leaf signature). Mirrors the chain-check portion of
/// `edgion_common::grpc_tls::validate_identity` but WITHOUT the validity-window
/// or private-key checks — this helper answers only "same trust root?" (spec:
/// emergency re-key / out-of-band BYO-root swap self-heal). Parse failures
/// return false — an unparsable cert cannot be trusted to chain.
pub fn chains_to(leaf_pem: &str, ca_pem: &str) -> bool {
    let Ok((_, leaf_doc)) = x509_parser::pem::parse_x509_pem(leaf_pem.as_bytes()) else {
        return false;
    };
    let Ok(leaf) = leaf_doc.parse_x509() else {
        return false;
    };
    let Ok((_, ca_doc)) = x509_parser::pem::parse_x509_pem(ca_pem.as_bytes()) else {
        return false;
    };
    let Ok(ca) = ca_doc.parse_x509() else {
        return false;
    };
    leaf.issuer() == ca.subject() && leaf.verify_signature(Some(ca.public_key())).is_ok()
}

/// Policy decision (spec §4). Policy triggers (`lifetime`/`scheduled`) evaluate the
/// CLIENT leaf only; the emergency net evaluates BOTH windows (spec §4.4).
/// For `scheduled`, both conjuncts evaluate AT the firing time, never at `now`
/// (spec §4.2 — evaluating the guard at `now` collapses the policy into a plain
/// remaining-threshold trigger that ignores the window).
///
/// Evaluation order: the configured policy conjunct is checked FIRST; the
/// emergency net runs only when the policy itself says NotDue/Manual. The
/// emergency net is a bottom-of-the-funnel safety net for schedules whose
/// normal trigger missed the window (spec §4.4's worst-case-alignment case),
/// not a preemptive check — under a healthy config `emergency_rotate_below <
/// rotate_if_remaining_below` (enforced by `validate_numeric`), so by the time
/// remaining drops to the emergency threshold the policy conjunct has almost
/// always already fired Due well earlier. Checking emergency unconditionally
/// first would mislabel an on-time scheduled/lifetime rotation as
/// `EmergencyDue` whenever the firing happens to land close to the leaf's
/// actual expiry (e.g. a weekly cron against a 365-day TTL that doesn't
/// divide evenly by 7 days).
pub fn rotation_due(cfg: &ConfSyncRotationConfig, i: &PolicyInputs, now: OffsetDateTime) -> RotationDecision {
    let (nb, na) = i.client_leaf;
    let policy_decision = match cfg.policy {
        RotationPolicy::Lifetime => {
            let validity = na - nb;
            let trigger = nb + validity * cfg.lifetime_ratio;
            if now >= trigger {
                Some(RotationDecision::Due("lifetime ratio reached"))
            } else {
                None
            }
        }
        RotationPolicy::Scheduled => match i.last_firing {
            // A `last_firing` that has not actually happened yet as of `now` cannot
            // have triggered anything — without this guard a stale/future firing
            // value (e.g. a caller-side bookkeeping bug) can flip the decision to
            // Due even though no real firing has occurred at-or-before `now`.
            Some(firing) if firing <= now => {
                let not_yet_acted_on = nb + BACKDATE_SLACK < firing;
                let due_at_firing = na - firing <= cfg.rotate_if_remaining_below.as_time();
                if not_yet_acted_on && due_at_firing {
                    Some(RotationDecision::Due("scheduled window due"))
                } else {
                    None
                }
            }
            _ => None,
        },
        RotationPolicy::Manual => None,
    };
    if let Some(decision) = policy_decision {
        return decision;
    }
    let emergency = cfg.emergency_rotate_below.as_time();
    if i.client_leaf.1 - now <= emergency {
        return RotationDecision::EmergencyDue("client leaf within emergency_rotate_below");
    }
    if i.server_leaf.1 - now <= emergency {
        return RotationDecision::EmergencyDue("server leaf within emergency_rotate_below");
    }
    RotationDecision::NotDue
}

/// CA renewal due-check (spec §4.5): remaining <= min(30d, 0.1 x ACTUAL validity).
pub fn ca_renewal_due(ca: (OffsetDateTime, OffsetDateTime), now: OffsetDateTime) -> bool {
    let validity = ca.1 - ca.0;
    let threshold = std::cmp::min(time::Duration::days(30), validity / 10);
    ca.1 - now <= threshold
}

fn resolve_tz(tz: &str) -> anyhow::Result<chrono_tz::Tz> {
    tz.parse::<chrono_tz::Tz>()
        .map_err(|_| anyhow::anyhow!("unknown IANA timezone '{tz}'"))
}

fn parse_cron(schedule: &str) -> anyhow::Result<croner::Cron> {
    schedule
        .parse::<croner::Cron>()
        .map_err(|e| anyhow::anyhow!("invalid cron expression '{schedule}': {e}"))
}

fn odt_to_chrono(t: OffsetDateTime, tz: chrono_tz::Tz) -> DateTime<chrono_tz::Tz> {
    tz.timestamp_opt(t.unix_timestamp(), 0).single().expect("valid unix ts")
}

fn chrono_to_odt(t: &DateTime<chrono_tz::Tz>) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(t.timestamp()).expect("valid unix ts")
}

/// Most recent firing at or before `now`. Backed by croner's native
/// `find_previous_occurrence` (inclusive=true so a `now` that lands exactly on
/// a firing counts as that firing) — no bounded backward search needed.
pub fn last_firing(schedule: &str, tz: &str, now: OffsetDateTime) -> anyhow::Result<Option<OffsetDateTime>> {
    let cron = parse_cron(schedule)?;
    let tz = resolve_tz(tz)?;
    let now_c = odt_to_chrono(now, tz);
    match cron.find_previous_occurrence(&now_c, true) {
        Ok(prev) => Ok(Some(chrono_to_odt(&prev))),
        // No previous occurrence within croner's search bound: never fired
        // (or the schedule never fires at all) — not an error for this query.
        Err(_) => Ok(None),
    }
}

/// Max gap between consecutive firings over the next 366 days (spec §4.2
/// feasibility check). None = the schedule never fires in the horizon.
///
/// Edge case (not exercised by the current test matrix, which only covers
/// daily/weekly/per-minute schedules — all of which fire many times within
/// 366 days): a schedule with EXACTLY ONE firing in the horizon yields
/// `prev = Some(_)` but `max_gap` stays zero (no second firing to diff
/// against). We fall back to a nominal 60s gap in that case rather than
/// reporting a zero gap (which would make `validate_schedule_controller`'s
/// `guard < 2 x max_gap` check meaningless).
pub fn max_cron_gap(schedule: &str, tz: &str) -> anyhow::Result<Option<std::time::Duration>> {
    let cron = parse_cron(schedule)?;
    let tz = resolve_tz(tz)?;
    let start = OffsetDateTime::now_utc();
    let horizon = start + time::Duration::days(366);
    let mut cursor = odt_to_chrono(start, tz);
    let horizon_c = odt_to_chrono(horizon, tz);
    let mut prev: Option<DateTime<chrono_tz::Tz>> = None;
    let mut max_gap = std::time::Duration::ZERO;
    // Cap iterations so a per-minute cron doesn't walk 500k firings: once we have
    // seen 10 000 firings the max gap is established for any practical pattern.
    for _ in 0..10_000 {
        let next = match cron.find_next_occurrence(&cursor, false) {
            Ok(n) if n <= horizon_c => n,
            _ => break,
        };
        if let Some(p) = prev {
            let gap = (next - p).to_std().unwrap_or_default();
            if gap > max_gap {
                max_gap = gap;
            }
        }
        prev = Some(next);
        cursor = next;
    }
    Ok(prev.map(|_| {
        if max_gap.is_zero() {
            std::time::Duration::from_secs(60)
        } else {
            max_gap
        }
    }))
}

/// Full controller-side rotation-config validation (spec §5.2): numeric invariants,
/// cron parse, tz resolve, never-fires reject; each entry in the returned `Vec` is one
/// independent warning (suspect-combination and/or expiry-watch WARN-band cross-warn)
/// the caller must WARN-log + `mark_system_error` on. An empty `Vec` means clean.
///
/// This also cross-checks against the expiry-watch WARN band (spec §6 Task 6, arbitrated
/// 2026-07-13, `edgion_common::conf_sync::cert_expiry_watch::WARN_THRESHOLD`): a healthy,
/// on-time rotation whose steady state still lands inside that band is not a bug — the
/// watch and the rotation config are two independently configured subsystems — but an
/// operator who tightened the expiry-watch band without also loosening the rotation guard
/// would otherwise "alert" on every single rotation cycle. This is advisory only: the
/// operator owns the tradeoff (per the operator-owns-config policy), so it warns rather
/// than rejects.
pub fn validate_schedule_controller(cfg: &ConfSyncRotationConfig) -> anyhow::Result<Vec<String>> {
    cfg.validate_numeric()?;
    let mut warnings = Vec::new();
    match cfg.policy {
        RotationPolicy::Lifetime => {
            let leaf = cfg.leaf_ttl.as_std();
            let remaining_at_rotation = leaf.mul_f64(1.0 - cfg.lifetime_ratio);
            if remaining_at_rotation < WARN_THRESHOLD {
                warnings.push(format!(
                    "lifetime policy rotates the leaf with only ~{}d remaining ({:.2}x of leaf_ttl {}), which is \
                     inside the expiry-watch WARN band (< {}d remaining): the watch will log a WARN transition \
                     every rotation cycle, right on schedule — this is expected alert-band time, not a failure. \
                     Lower lifetime_ratio (rotate earlier in the leaf's life) if you'd rather stay clear of the \
                     WARN band.",
                    remaining_at_rotation.as_secs() / 86_400,
                    1.0 - cfg.lifetime_ratio,
                    cfg.leaf_ttl,
                    WARN_THRESHOLD.as_secs() / 86_400,
                ));
            }
        }
        RotationPolicy::Scheduled => {
            let schedule = cfg.schedule.as_deref().expect("validate_numeric guarantees schedule");
            let gap = max_cron_gap(schedule, &cfg.schedule_timezone)?.ok_or_else(|| {
                anyhow::anyhow!("schedule '{schedule}' never fires within 366 days — almost certainly a typo")
            })?;
            let guard = cfg.rotate_if_remaining_below.as_std();
            if guard < gap * 2 {
                warnings.push(format!(
                    "rotate_if_remaining_below ({}) is < 2x the max firing gap ({}s): a due window \
                     may be skipped and rotation can fall back to the emergency path",
                    cfg.rotate_if_remaining_below,
                    gap.as_secs()
                ));
            }
            if guard < WARN_THRESHOLD {
                let worst_case_in_band = WARN_THRESHOLD.saturating_sub(guard) + gap;
                warnings.push(format!(
                    "rotate_if_remaining_below ({}) is below the expiry-watch WARN threshold ({}d): each \
                     rotation cycle, the leaf can spend up to ~{}h inside the expiry-watch WARN band before the \
                     scheduled rotation actually fires (WARN_THRESHOLD - guard, plus up to one firing gap of \
                     {}h). This is expected alert-band time, not a misconfiguration — set guard >= 720h if you'd \
                     rather stay clear of the WARN band. (Note: the DEFAULT scheduled guard is 336h, which \
                     triggers this warning; that is intended and informational, not an error.)",
                    cfg.rotate_if_remaining_below,
                    WARN_THRESHOLD.as_secs() / 86_400,
                    worst_case_in_band.as_secs() / 3600,
                    gap.as_secs() / 3600,
                ));
            }
        }
        RotationPolicy::Manual => {}
    }
    Ok(warnings)
}

#[cfg(test)]
mod tests {
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
}
