---
name: resource-change-notification-gating
description: ACME notification deduplication uses published Controller results, independently of status CAS success.
knowledge_type: regression-guards
---

# Resource change notification gating

## Trigger and correction

The old gate compared the raw object's persisted Accepted generation and
ResolvedRefs with the newly computed conditions. When the ACME service won a
status write before the generic resourceVersion-guarded PATCH, that PATCH failed
and a later status-only WATCH still appeared to be a meaningful change. Repeated
notifications could reset the service retry budget and sustain a feedback loop.

`EdgionAcmeHandler::parse` now compares UID, generation, and ResolvedRefs
status/reason against the previous **processed ServerCache** object. The serial
resource worker publishes each result before processing another event, regardless
of whether status persistence subsequently succeeds. An absent previous object
or a changed identity/condition requests one notification; a lifecycle-only echo
does not. A missing generation compares equal to a previous missing generation.

The request is a per-reconcile `notify_after_publish` flag, reset at parse entry
and excluded from both serde input/output and schemars. `after_publish` sends the
notification only after the new conditions and attachments are visible. Sending
during parse could let the service consume a recovery notification while its
eligibility gate still sees the previous authoritative denial, losing recovery.
Corrupt credentials and unresolved Gateways still publish their condition without
requesting issuance; a later condition recovery is compared with that cached result.

No extra store, queue, timer, status writer, or API call is introduced. Startup
scanning and explicit Admin retry retain their existing independent notifications.

## Regression evidence and limits

- `handlers/tests/edgion_acme_test.rs` exercises the real ResourceProcessor and
  captures actual ResourceChanged channel sends without starting an ACME worker.
  It covers repeated stale raw conditions with Issuing/Failed/challenge echoes,
  input flag reset, UID/generation changes, missing generations, credential
  recovery, and cache visibility before the notification hook.
- The EdgionAcme resource transport test checks schema omission, serialization
  omission, and rejection of an input attempt to set notification intent.
- This does not simulate an API server or live certificate issuance. The status
  echo test deliberately withholds computed conditions to model CAS rejection;
  the generic persistence tests separately exercise actual conflict responses.
- Existing notification semantics still do not identify a valid-to-valid account
  credential rotation or a listener change whose ResolvedRefs status/reason stays
  unchanged. Service scheduler backoff, timer deduplication, and GET-error policy
  are outside this correction; this guard is not proof against every ACME loop.
- Notification deduplication does not itself gate generic status writes. The
  separate `acme_is_owned` handler/service admission check now prevents foreign
  GatewayClasses from writing competing flat handler conditions. Active attempts
  that outlive ownership transfer are outside this correction; admission is not
  a strict exactly-once issuance proof.

Fresh targeted verification for this correction:

- `cargo test -p edgion-controller --lib acme -- --test-threads=2`: 32 passed.
- `cargo test -p edgion-resources --lib resources::edgion_acme`: 10 passed.
- `cargo check -p edgion-controller --all-targets`: passed.
- Changed Rust files formatted with rustfmt; `git diff --check`: passed.
- Independent implementation review: LGTM. Workspace-wide checks remain the
  responsibility of the encompassing status-loop fix batch.

## Resolution references

- `c1-013` — —.
- `CAND-P3-01` — status-CAS notification feedback correction.
