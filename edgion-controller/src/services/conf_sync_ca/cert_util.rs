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
#[path = "../../../tests/unit/services/conf_sync_ca/cert_util_test.rs"]
mod tests;
