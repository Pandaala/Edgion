---
name: geoip-validators-bound-to-update-source
description: Use when reviewing GeoIP updater restart validators, source changes, credential-dependent representation identity, or absent-sidecar mtime recovery.
knowledge_type: regression-guards
finding_id: ARCH-015
---

# GeoIP validators are bound to the configured update source

## Conclusion and scope

The gateway-local GeoIP updater persists ETag/Last-Modified together with an opaque
source identity. Conditional reuse requires matching identity plus the existing
regular durable database and active provider checks. A restart from source A to
source B cannot reuse A's validators even when both sources legitimately use the
same ETag. Credential changes also invalidate conditional reuse.

The identity is calculated once at `GeoIpUpdater::new` from the exact configured
URL and resolved Authorization value, using a domain-separated, length-framed
SHA-256 digest. The environment variable name is not part of representation identity.
URL textual differences conservatively trigger an unconditional download. Plaintext
URLs and credentials are never persisted in the sidecar; neither inputs nor identity
are logged. Header sensitivity, canonical outbound client/egress and URL-stripped
error handling remain unchanged.

## Rationale and evidence

ARCH-015 identified that `UpdateState` persisted only validators beside `db_path`.
Restarting with another URL recovered them solely on local database usability.
Entity tags are scoped to representations, so equal tags across different sources
could preserve the wrong database indefinitely through successful 304 cycles.

[Updater ownership](../../../../edgion-gateway/src/geoip/updater.rs) now separates
cold source identity from mutable conditional state. `UpdateState::load` clears
validators for mismatched or identity-less existing metadata and preserves the old
or absent identity. Malformed or unreadable existing metadata stays unbound and
unconditional. Both existing request eligibility checks require source matching.
Only `record_swap`, called after structural validation, rename and successful
provider publication, promotes state to the current source and response validators.
An invalid 304 clears validators without promoting source identity.

[Unit regressions](../../../../edgion-gateway/tests/unit/geoip/updater_test.rs) cover
same-source restart and ETag precedence; different URLs sharing a tag; repeated
mismatch/restart; legacy, malformed and unreadable sidecars; resolved credential
identity and plaintext exclusion; surviving old metadata after failed persistence;
and new-source promotion. A disk-precheck failure exercises the real cycle path
without network, preserving the last-good provider and bytes across repeated
attempts and reloads. Existing cycle coverage asserts source identity persistence
after successful rename/provider swap.

## Alternatives and limits

Always-unconditional updates would fix source confusion but discard same-source
restart bandwidth benefits. No new cache, registry, full-body buffer or config knob
is needed. Existing streaming bounds, deadlines, free-space policy, mmap lifetime,
backoff and last-good serving remain the governing behavior.

Only a genuinely absent sidecar with usable local state retains database-mtime
recovery. This deliberately assumes that database belongs to the configured source;
external deletion removes provenance and cannot establish a source mismatch. Existing
untrusted metadata never receives this fallback. See the
[operator policy](../../../../docs/en/ops-guide/gateway/geoip.md#multi-replica-update-behavior-jitter-and-the-meta-validator).

The digest is equality metadata, not encryption or protection against offline
credential guessing. The sidecar remains trusted local state. Persistence is
best-effort and is not a crash-consistent database/metadata transaction. A surviving
old-source sidecar or malformed write remains unconditional; absent metadata retains
the explicit mtime assumption above. The change does not establish consistency under
multiple independent writers sharing one database path.

## Verification and re-evaluation

Fresh coordinator verification passed:

- `cargo test -p edgion-gateway --lib geoip::updater::tests`: 38 passed, 0 failed;
  all 31 existing test names were preserved and 7 new regressions were added.
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets`: passed; clippy reported no warnings.
- Agent-doc consistency, SSA force, metrics inventory, Pingora source policy,
  Gateway API transform contract (`--check`), unit-test layout, gateway no-tracing,
  English-content guards, and `git diff --check`: passed.

Newly added tests do not start servers or send network requests. Existing historical
network cycle tests are retained with their identities. The full test suite was not
run. Pure state promotion tests do not claim live interoperability for two HTTP
sources; no new two-source integration execution is claimed.

Re-evaluate if representation-affecting inputs are added, redirects become supported,
credentials are refreshed dynamically, metadata storage policy changes, or multiple
writers are supported. Preserve the rule that a failed replacement never promotes
source provenance or changes existing last-good traffic policy.

## Resolution references

- ARCH-015: source identity missing from persisted GeoIP validators; bounded correction
  adds restart source matching and maintains the deliberate absent-sidecar policy.
