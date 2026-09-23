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

`EdgionAcmeHandler::parse` compares UID, generation, ResolvedRefs status/reason,
and resolved listener attachments against the previous **processed ServerCache** object. The serial
resource worker publishes each result before processing another event, regardless
of whether status persistence subsequently succeeds. An absent previous object
or a changed identity/condition requests one notification; a lifecycle-only echo
does not. A missing generation compares equal to a previous missing generation.

The request is a per-reconcile `notify_after_publish` flag, reset at parse entry
and excluded from both serde input/output and schemars. `after_publish` sends the
notification only after the new conditions and attachments are visible. Sending
during parse could let the service consume a recovery notification while its
eligibility gate still sees the previous authoritative denial, losing recovery.
Corrupt credentials and unresolved Gateways still publish their condition and
notify after publication. The actor's eligibility check refuses replacement work,
while the notification cancels pre-issuance work admitted under the old proof.
After issuance, the retained certificate's publication gate distinguishes a
fresh proof for the same serving selection from revoked authority; this avoids
restarting issuance after the output Secret reloads the Gateway. Deletes also
notify so the next authority check rejects the removed source.

Notification gating introduces no API call. Startup scanning and explicit Admin
retry retain independent commands; the actor coalesces all command sources.

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
- Secret dependency requeues and listener-proof comparison cover serving changes
  and the corrupt-credential and credential-recovery transitions. The gate compares
  the `resolved_refs_identity` status/reason pair, so a change in either member
  requests a notification: valid/corrupt flips the status, while a denial-to-corrupt
  move such as `RefNotPermitted` to `DecodeError` changes only the reason.
- Valid-to-valid account credential rotation intentionally emits no notification.
  UID, generation, ResolvedRefs status/reason, and listener attachments are all
  unchanged, so `notify_after_publish` stays false and no immediate recheck is
  requested. This is the intended behavior rather than a residue of the status-loop
  correction: the account Secret is never attached to the in-memory resource, so the
  next admitted attempt re-reads it from the API server and uses the rotated
  credential. An attempt already in flight keeps the credential it loaded and may
  complete against the pre-rotation account; the rotated value takes effect from the
  next admitted attempt. A reprocess whose gate inputs are unchanged emitting no
  notification is asserted by the tail of
  `recovered_credentials_are_published_before_the_single_notification`.
- Notification deduplication does not itself gate generic status writes. The
  separate `acme_is_owned` handler/service admission check now prevents foreign
  GatewayClasses from writing competing flat handler conditions. Active attempts
  additionally revalidate ownership under the
  [scheduler fencing guard](acme-scheduler-fencing-recovery.md); this still does
  not provide exactly-once CA issuance.

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
- `acme-scheduler-trigger-admission` — active-attempt cancellation event source;
  intended valid-to-valid account credential rotation behavior.
