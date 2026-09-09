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

Historical B0195 targeted runs passed: `cargo test --locked -q -p edgion-gateway --lib
session_persistence -- --test-threads=1` (21 tests, including 19 in this module),
and the same command with the `cookie_value` filter (4 tests).

The D0109 follow-up adds cross-process coverage to the existing HTTPRoute and
GRPCRoute `Backend/SessionPersistence` suites:

- `cookie_fields_and_malformed_expiry` sends two separate Cookie fields over
  HTTP/1.1; `grpc_sp_cookie_fields_and_malformed_expiry` appends separate Cookie
  metadata fields over HTTP/2. Both put an unrelated cookie first and the issued
  affinity cookie second, assert the issuing backend's echoed identity, and
  reject any affinity Set-Cookie refresh on ten replays.
- Both cases first send twenty token-less requests and require at least two
  distinct backend identities. HTTP adds EndpointSlices for the shared server's
  existing ports 30002/30003 alongside 30001; the existing Service has no
  targetPort. gRPC reuses its existing three-port topology. This guards against
  a single-backend setup making a broken affinity implementation pass.
- Both append nonnumeric, empty, overflowing-u64, and extra-dot expiry suffixes
  to a known live backend hash. Each response must issue exactly one affinity
  cookie with a valid future expiry; five replays must pin that response's
  backend without another refresh. A refreshed token may have the same bytes
  within one second, so refresh is established by Set-Cookie presence rather
  than token inequality.
- Requests and RPCs have bounded timeouts; tests reuse the shared topology and
  add no sleeps or process restarts. Production structs, schema, token grammar,
  routing, and hot-reload behavior are unchanged.

Current D0109 verification (2026-09-09, macOS local shared topology):

- `./edgion-tests/integration/scripts/utils/prepare.sh` passed; it built the
  canonical binaries and precompiled required ignored test executables without
  running the full unit-test suite.
- `./edgion-tests/integration/scripts/integration/run_integration.sh --no-prepare
  -r HTTPRoute -i Backend/SessionPersistence`: 5/5 cases passed, including the new
  wire case. Report: `tmp/integration_testing/testing_20260909_154437/report.log`.
- The same runner with `-r GRPCRoute -i Backend/SessionPersistence`: 5/5 cases
  passed, including the new wire case. Report:
  `tmp/integration_testing/testing_20260909_154516/report.log`.
- Both completed reports have `Status: PASS`, zero failures, and passing final
  metrics-label, unknown-field, requeue-loop, and cache-residue gates.
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets` passed. Existing Clippy warnings
  remain outside the added code. Agent-doc validation, SSA-force, metrics
  inventory, and Pingora source-policy guards passed.
- Independent read-only review approved the test logic and checked the locked
  reqwest/Hyper and tonic/h2 encoding paths for repeated Cookie field retention.

Re-evaluate if cookie parsing, token grammar, affinity map keys, or expiry
semantics change. Local wire evidence does not establish Kubernetes admission
conformance or behavior through an intermediary that rewrites Cookie fields.

## Resolution references

- B0195: HTTP session persistence file optimization.
- D0109 / `add-session-cookie-field-integration`: HTTP/gRPC wire coverage completed
  on 2026-09-09; production behavior unchanged. The existing rule is extended,
  not replaced by a separate finding archive.
