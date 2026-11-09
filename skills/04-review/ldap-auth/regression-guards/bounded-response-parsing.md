---
name: ldap-auth-bounded-response-parsing
description: Use when reviewing LDAP Auth response parsing, BER decoding, panics or stack overflow on malformed directory replies, frame-size limits, StartTLS stream handoff, or proposals to reintroduce ldap3/lber, add recursion or a buffered reader to edgion-ldap, or wrap the bind in catch_unwind.
status: fixed
finding_id: auth-01, auth-02
closed: 2026-11-09
knowledge_type: regression-guards
---

# LDAP Auth Bounded Response Parsing — FIXED (auth-01, auth-02)

## Conclusion and scope

`ldap_auth` decodes every LDAP reply with the in-tree `edgion-ldap` leaf crate.
No server-controlled byte sequence may panic the request task or grow the stack.
Every malformed reply ends as a fail-close 503 through the existing
`LdapAuthError` outcomes:

- The codec (`edgion-ldap/src/codec.rs`, `proto.rs`) is a flat cursor with
  straight-line decoders of fixed depth (LDAPMessage, protocolOp, LDAPResult).
  **No function in the crate is directly or indirectly recursive.**
- An inbound frame is limited to `MAX_FRAME` (64 KiB). The limit is checked from
  the at most six header bytes, before any allocation or body read.
- Only single-octet identifiers and definite lengths of at most four octets are
  accepted. Non-minimal long-form lengths (emitted by OpenLDAP liblber) are
  accepted.
- `matchedDN` and `diagnosticMessage` are validated as primitive UTF-8 OCTET
  STRINGs and discarded. Only the result code leaves the decoder, so no peer text
  reaches logs or response bodies. Optional trailing elements (referral,
  serverSaslCreds, responseName/Value, controls, and a trailing `[10]` element
  with the shape of Active Directory's misplaced Notice-of-Disconnection OID)
  are skipped by length without descending. A real Notice of Disconnection
  carries message ID 0 and is rejected (fail-closed) like any unsolicited
  notification.
- A reply whose message ID does not answer the outstanding request (including the
  unsolicited ID 0) is a protocol error.
- **StartTLS invariant:** `BindSession` reads exactly one frame per response and
  never reads ahead (no buffered reader, no speculative read). Bytes after the
  ExtendedResponse must reach the TLS handshake untouched.

The plugin (`edgion-gateway/src/plugins/http/ldap_auth/plugin.rs`) keeps dialing
through `EgressResolver`, builds the rustls `ClientConfig` with
`build_ldap_rustls_config`, and performs the LDAPS / StartTLS handshake itself with
`tokio-rustls`. Every `edgion_ldap::Error` maps to a static outcome:

| Phase | Failure | Outcome |
|---|---|---|
| bind | any decode or stream error, including a post-handshake TLS alert (for example a TLS 1.3 client-certificate rejection) | `BindFailed` (`ldap bind fail`, `bind_fail`) |
| StartTLS | decode error, `rc != 0`, peer close | `ConnectionFailed` (`ldap unreachable`, `unreachable`) |
| TLS handshake (client-side `connect` future) | SAN pinning rejects | `TlsSanMismatch` |
| TLS handshake (client-side `connect` future) | anything else | `ConnectionFailed` |
| either phase | deadline elapsed | `Timeout` |

The two-phase timeout of the former implementation is kept. Phase 1 covers dial,
StartTLS and the TLS handshake. Phase 2 covers the bind and a best-effort unbind
(UnbindRequest, then a write-side shutdown, which sends TLS close_notify as
ldap3 did) under the same deadline; the unbind never changes the outcome.

## Rationale and evidence

Findings from the 2026-11-08 Gateway data-plane panic audit:

- **auth-01 (Medium).** `ldap3` 0.12.1 decoded LDAPResult with `expect` /
  `panic!` in the request task (`ldap3-0.12.1/src/result.rs:341-391`, called
  from `ldap.rs:196`). Driver-task variants panicked in `protocol.rs:71-109`. A
  malformed BindResponse unwound the downstream connection instead of producing
  the 503.
- **auth-02 (Medium, process abort).** `lber` 0.5.1 `parse_tag` recursed once per
  constructed element with no depth limit (`lber-0.5.1/src/parse.rs:54-85`), and
  `ldap3` applied it with no frame cap (`protocol.rs:52-57`). A ~1 MB nested
  reply overflowed the worker stack and aborted the whole process; unwinding and
  `catch_unwind` cannot contain that.

Regression vectors, server to gateway, as the reply to bind message ID 1. Each is
asserted with its exact `edgion_ldap::Error` in
`edgion-ldap/tests/unit/proto_test.rs` and as `BindFailed` through the plugin in
`edgion-gateway/tests/unit/plugins/http/ldap_auth/plugin_test.rs`:

| Bytes | ldap3/lber | edgion-ldap |
|---|---|---|
| `30 05 02 01 01 61 00` (empty BindResponse) | request-task panic | `Truncated` |
| `30 0d 02 01 01 61 08 0a 01 00 04 01 ff 04 00` (non-UTF-8 matchedDN) | request-task panic | `InvalidUtf8` |
| `30 05 02 01 01 41 00` (primitive protocolOp) | request-task panic | `UnexpectedTag{0x61,0x41}` |
| `30 00` | driver-task panic | `Truncated` |
| 64 × `30 83 L2 L1 L0` around `02 01 01` (declares 318 bytes) | recursion | `UnexpectedTag{0x02,0x30}` |
| 200 000 × the same (declares 999 998 bytes) | stack overflow, abort | `FrameTooLarge` after 5 bytes |

The non-UTF-8 matchedDN vector is well-formed BER. It fails closed because
RFC 4511 `LDAPString` is UTF-8 (user decision, 2026-11-09). Consequence: a
non-conforming server that returns rc 49 with a non-UTF-8 diagnosticMessage
(for example Latin-1) yields 503 `ldap bind fail`, not 401; ldap3 panicked on
the same bytes.

## Alternatives and limits

Rejected in the former task `auth-01-02-edgion-ldap-minimal-bind-client`
(2026-11-08; deleted on completion, never committed):

- **`catch_unwind` around the bind.** It cannot contain the auth-02 abort, and
  ldap3 panic payloads carry peer bytes.
- **A `[patch.crates-io]` fork of ldap3/lber.** It would mean about 7 800 lines,
  of which Edgion uses under 5%.
- **`ldap-rs`, `ldap-client`.** They resolve and connect by themselves, which
  breaks dial-time egress admission
  ([dial-time-egress-policy](dial-time-egress-policy.md)).
- **`ldap3_client`.** It is the same `lber` lineage.

Behavior deltas accepted with the fix, all still fail-closed:

| Case | Before | After |
|---|---|---|
| Malformed or non-UTF-8 reply | panic or abort | 503 `ldap bind fail` / `ldap unreachable` |
| Unmatched message ID during bind | dropped; wait for close or phase-2 timeout | immediate `BindFailed` |
| Unmatched ID or peer close during StartTLS | phase-1 `Timeout` (`ldap timeout`) | immediate `ConnectionFailed` (`ldap unreachable`) |
| `unbind` | unbounded | bounded by the phase-2 deadline |
| Bare IPv6 `ldapHost` (`::1`) | `Url::parse` failed on every request | dialed through `EgressResolver` |
| Detached driver task | one `ldap3::drive!` task per request | none |

A TLS `ldapHost` that `ServerName` rejects is parsed once at construction and
still fails per request as `ConnectionFailed`. Failing construction instead
would turn the whole EdgionPlugins resource into a 500 `RejectFilter` with no
`ldap_auth` metric (`plugin_runtime.rs` "plugin construction failed" →
`seal_fail_closed`).

The bounded-parsing unit tests drive the plugin through a `cfg(test)` in-memory
stream seam (`install_scripted_stream`), not loopback sockets. This follows the
no-network rule and the precedents in
[JWKS unit tests without network](../../oidc/regression-guards/jwks-unit-tests-without-network.md)
and [request-mirror unit tests without network](../../request-mirror/regression-guards/unit-tests-without-network.md).
Release builds contain no seam field, lock, or branch. Two older dial tests in the same file
(`dial_skips_refused_candidate_and_uses_next`, `all_refused_candidates_fail_closed`)
still use loopback addresses; moving them behind a seam is tracked in
`tasks/todo/ldap-auth-follow-ups.md`.

The `ldap` cargo feature was removed. LDAP now compiles unconditionally because
`edgion-ldap` adds no third-party dependency
([optional subsystem features](../../packaging/intentional-design/gateway-optional-subsystem-features.md)).

**Open limitation: interoperability.** OpenLDAP is covered by the integration
suites. No Active Directory or Samba AD DC bind (rc 0, rc 49 with the `data 52e`
diagnostic) has been run against this implementation; no environment was
available on 2026-11-09. AD-specific tails are skipped by length; a Notice
of Disconnection (message ID 0) fails the bind closed.

## Verification and re-evaluation

Fresh results on 2026-11-09, `feature-10-28`:

- `cargo test -p edgion-ldap`: 33 passed.
- `cargo test -p edgion-gateway --lib ldap_auth`: 41 passed.
- Mutation checks, each caught by a test:
  - a recursive `skip_element` overflows the stack in
    `deep_nesting_within_frame_cap_is_skipped_without_descent` (a 16 000-level
    nest inside the 64 KiB cap, decoded on a 128 KiB thread), in both debug
    and `--release` test builds;
  - removing the frame cap fails `oversized_declared_length_leaves_body_unread`
    instead of hanging;
  - removing the phase-2 deadline around `unbind`, or giving the unbind a fresh
    budget, fails `phase2_deadline_bounds_unbind`;
  - mapping the phase-2 deadline to `BindFailed` fails
    `phase2_timeout_when_peer_never_replies`.
- `cargo test -p edgion-gateway --lib plugin_runtime`: 129 passed.
- `cargo tree -i ldap3` / `-i lber`: no package matched.
- `cargo check --workspace --all-targets` and the core build
  (`--no-default-features --features allocator-jemalloc,boringssl`) passed.
- cargo-fuzz `ldap_decode`: 64 746 774 executions in 61 seconds, no crash
  (recorded in `edgion-ldap/tests/fuzz/README.md`).
- Integration against real OpenLDAP containers: `run_integration.sh --full-test
  -r EdgionPlugins -i LdapAuthTls` passed 5/5 (private CA, wrong CA, mTLS with and
  without client certificate, StartTLS with custom CA);
  `test_ldap_auth_egress.sh` passed 36/36 (LDAPS and StartTLS, two rounds).
- `run_integration.sh --full-test -r EdgionPlugins -i LdapAuth` (plain OpenLDAP)
  passed 10/10, including valid bind, wrong password (401), anonymous mode, and
  the space and `+` DN-escape round trips. One earlier run failed only
  `anonymous_access_sets_headers` with 404 `RouteNotFound`: the controller
  processed that HTTPRoute before its parent Gateway and never re-resolved its
  listeners. That is a route-attachment ordering race outside the LDAP path.

Re-evaluate when any of these change:

- A new search or response kind is added. Search mode (2026-11-09) re-evaluated
  this page: search frames (entry, reference, done) decode with the same
  straight-line cursor, at most 2 entries and 8 references per search, each
  frame bounded by `MAX_FRAME`; the retained-text rule now allows exactly one
  matched DN and one identity value, never logged
  ([search mode guards](search-mode-identity-and-filter.md)). Group
  requirements (Phase 2b, 2026-11-09) reuse the same `search_unique` loop with
  base scope, sizeLimit 1 and attribute `1.1` (no values retained); the group
  filter is operator configuration, pre-encoded once.
- `MAX_FRAME` changes.
- A buffered reader or speculative read is introduced in `client.rs`.
- Any recursive helper is introduced in `edgion-ldap`.
- The LDAP client starts dialing or resolving by itself.
- An AD/Samba interoperability run becomes available.

## Resolution references

- `auth-01`: malformed BindResponse / ExtendedResponse panics the request task
  (panic audit 2026-11-08). Deferred and promoted to a task on 2026-11-08; fixed
  2026-11-09.
- `auth-02`: nested BER overflows the stack and aborts the gateway (panic audit
  2026-11-08). Deferred and promoted to a task on 2026-11-08; fixed 2026-11-09.
