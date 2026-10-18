# ResponseRewrite rename value ownership and fidelity

Status: deferred implementation scope for user decision. No runtime change.
Source baseline: 0f56897e5.

## Current evidence

ResponseRewrite::modify_headers checks both rename names against the canonical
response protection policy, then calls get_response_header for the source.
PingoraSessionAdapter reads HeaderMap::get, validates that first value with
HeaderValue::to_str, and copies it into String. It does not search later values
when the first value fails text conversion. Without a bound response, the getter
returns None; pending operations are not consulted.

The string destination setter parses that text into a new HeaderValue before
the canonical typed write. The locked http 1.5.0 HeaderValue constructors set
is_sensitive to false. A sensitive textual source therefore loses that metadata
through this round trip. This is conditional source-level metadata loss, not a
reproduced production secret exposure. HeaderValue sensitivity affects cooperative
Debug and protocol consumers; it is not encryption or memory erasure.

After successful destination set, the source name is removed. Destination set
failure preserves the source; source removal failure leaves the destination
visible and logs an error without rollback. Following rename/add/set/remove
operations continue under the existing policy. Source duplicates are not all
copied: the getter selects one value and successful removal removes the name.

## Why this is separate from automatic performance work

A typed value clone could avoid the intermediate String and text reparse, but it
also retains sensitivity metadata. Copying raw values without the existing text
gate would additionally change which responses are renamed. The user's automatic
optimization scope requires unaffected behavior, so this coupled metadata
correction is recorded explicitly rather than hidden in an allocation change.

Preserving known sensitive metadata is a required project boundary, not a
choice to keep dropping flags for compatibility. Any selected implementation
must retain it; this task does not accept the current loss as intended design.
The user decision concerns scheduling and exact fidelity scope, particularly
whether opaque-value support belongs in the same change.

## Bounded implementation choices

- Use the existing live response map and typed destination writer, retaining
  first-value selection and the to_str eligibility gate while cloning the
  eligible HeaderValue with its sensitive flag. Do not clear flags to mimic
  the existing string round trip.
- Consider opaque value support separately with an explicit product decision.
  ExtProc's raw-value contract does not automatically authorize expanding
  ResponseRewrite semantics. Copying all source values, atomic rename, or
  changing overwrite/remove order are separate changes too.

Values must be acquired at each rename's execution point so chained renames see
prior mutations. Hold an owned typed value before mutating the session, but do
not pre-copy the entire response map. Keep canonical protected-header checks
and the distinction between bound response state and queued operations.
No new name parser, local redaction rule, per-request metric or tracing is needed.

A dynamic HeaderValue clone can allocate on its first byte-storage promotion.
The prepared fixed-value optimization does not establish zero allocations for
dynamic upstream headers. Measure short/long and fresh/shared source values
separately; compare the removed String/reparse work without a throughput claim.

## Required verification

Add real-adapter cases with sensitive and ordinary textual values, empty values,
multiple source values, opaque first plus textual second, missing source and no
bound response. Assert bytes, is_sensitive, duplicate behavior and destination
overwrite explicitly; semantic HeaderValue equality alone ignores sensitivity.
Test chained renames, both protected names, set failure, source-remove failure,
continued processing, static logs and configuration self-rename rejection.
If opaque support is selected, add explicit acceptance for those changed cases.

Run focused ResponseRewrite/session-adapter tests and required repository gates
under a separate IMPL plan and independent final review. Freeze the source
allow-list then. This record is source inspection only: no runtime reproducer,
Rust test or performance benchmark was run for this documentation batch.

Relevant original knowledge:
- skills/04-review/security/mandatory-design/secret-header-values-must-be-sensitive.md
- skills/04-review/plugin-runtime/regression-guards/response-header-mutation-policy.md
- skills/04-review/ext-proc/regression-guards/opaque-header-fidelity.md
