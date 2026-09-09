---
name: session-persistence-token-extraction
description: Review affinity cookie extraction across repeated fields and malformed wire-token expiry handling.
knowledge_type: regression-guards
---

# Session persistence token extraction

## Conclusion and scope

HTTP and gRPC session persistence share `sp_extract_token` in
`edgion-gateway/src/routes/http/session_persistence.rs`. Cookie extraction uses
the canonical `plugins::runtime::session_adapter::cookie_value` helper, which
searches all Cookie fields in order and returns the first matching cookie.
Malformed expiry suffixes remain part of the lookup token; they cannot be
stripped to adopt an existing endpoint's bare hash.

## Rationale and evidence

B0195 found two extraction defects: a token in a later Cookie field was ignored,
and a token such as `<known-hash>.invalid` was reduced to `<known-hash>` without
expiry. The shared cookie helper fixes field traversal without introducing
another parser. `parse_wire_token` preserves the original token on numeric
parse failure. Affinity maps contain generated hexadecimal hashes, so the
retained dot and invalid suffix cause a miss and the existing resolver refreshes
the session. Valid expiry still expires when `now >= expiry`; valid live expiry
is removed with `String::truncate`, reusing the extraction buffer.

## Alternatives and limits

The canonical parser's exact-name and value semantics replace the former local
whitespace trimming. First-match behavior includes empty values; an invalid
UTF-8 field before a match aborts lookup, while later fields do not override an
earlier match. The shared helper owns these semantics.

This is an affinity hint correction, not an authentication mechanism: tokens
remain unkeyed, and clients can still supply bare hashes. See the
[affinity threat model](../../lb/intentional-design/crc32-session-affinity-not-a-security-boundary.md).
The [strict commitment boundary](sp-strict-mode-fail-close.md) remains unchanged:
a lookup miss establishes a new session, while strict enforcement applies to
a resolved target. Backend selection, retries, and response injection are unchanged.

## Verification and re-evaluation

Target unit tests exercise actual Pingora H1 request parsing over an in-memory
duplex stream, later Cookie fields, duplicate precedence, malformed nonnumeric,
empty, overflowing and extra-dot expiry suffixes, live/bare/expired tokens,
the expiry equality boundary, and owned-buffer reuse. The existing rule-scoped
lookup test also verifies a malformed token misses despite a known live hash.
Canonical `cookie_value` tests cover malformed-field handling.

Fresh targeted runs passed: `cargo test --locked -q -p edgion-gateway --lib
session_persistence -- --test-threads=1` (21 tests, including 19 in this module),
and the same command with the `cookie_value` filter (4 tests).

Cross-process HTTP/gRPC validation is deferred to
[integration coverage](../../../../tasks/todo/add-session-cookie-field-integration.md).
Re-evaluate if cookie parsing, token grammar, affinity map keys, or expiry
semantics change. Unit evidence does not establish cross-process conformance.

## Resolution references

- B0195: HTTP session persistence file optimization.
