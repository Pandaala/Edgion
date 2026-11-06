---
name: duplicate-host-rejected
description: Pingora rejects duplicate HTTP/1.1 Host fields before Gateway request filters; no operator switch can permit them.
knowledge_type: regression-guards
---

# Duplicate HTTP/1.1 Host fields are rejected by Pingora

## Current contract

The supported Pingora fork rejects an HTTP/1.1 request with more than one
`Host` field during protocol parsing, before an Edgion HTTP or redirect filter
runs. This is the RFC 9110 section 7.2 outcome. Edgion has no setting that
permits duplicate Host fields. A rejected request cannot reach route matching,
plugins, or an upstream. Protocol-level rejections may not have an Edgion
request context or access-log diagnostic.

The fork also validates userinfo and ambiguous authority representations at
ingress and checks standard proxy egress after filter mutations. The authority
in an absolute-form target must agree with Host. The core treats HTTP/2
authority failures as stream-local. These are separate from Edgion's
`NormalizedHostname` route-selection view and must not be replaced with a
second Gateway authority parser.

## Root cause and correction

The former `SecurityProtectConfig.reject_duplicate_host` setting and the
corresponding request-context checks ran after Pingora had already rejected a
duplicate Host. Thus its `false` value could not provide the documented opt-out.
P090-003 removed the setting, the duplicate Gateway checks, and the associated
unreachable `DuplicateHostHeader` diagnostic. The protocol layer owns this
rejection for both ordinary and redirect listeners.

## Verification and limits

- Fork source: `pingora-core/src/protocols/http/authority.rs` and
  `v1/server.rs` at `edgion_v0.9.1@e5f4287be713d01ffd444544251bf75c9d00062a`.
  Re-verified by source inspection at that revision on 2026-11-06 after the
  fork was rebased onto upstream 0.9.1: `validate_request_authority_fields`
  rejects a second `Host` ("multiple Host header fields"),
  `HttpSession::validate_request` calls `validate_request_authority` from
  `read_request`, HTTP/2 ingress calls the shared field check stream-locally
  (`v2/server.rs`), and `proxy_h1.rs`/`proxy_h2.rs` re-run
  `validate_request_authority` on egress. `authority.rs` is unchanged from the
  earlier verified revision `0b0d8bba`; the `v1/server.rs` delta is limited to trailer write timeouts.
- Historical: at the earlier verified revision `0b0d8bba9609655bfd7db0fe0add6818bc0ee937`
  (`edgion_0.9.0_v1`), `cargo test -p pingora-http --lib` passed 56 tests,
  `cargo test -p pingora-core --lib authority` passed 16 tests, and the
  `pingora-proxy/tests/test_upstream.rs` ambiguous-authority and post-filter
  userinfo cases passed against their own origin captures. The proxy tests
  used a temporary local health listener for the unrelated OpenResty startup
  check; no fork source was changed.
- Edgion consumer tests: `edgion-gateway/tests/unit/routes/http/proxy_http/pg_request_filter_test/xff_snapshot_tests.rs`,
  `edgion-gateway/tests/unit/routes/http/redirect_http_test.rs`, and
  `edgion-tests/src/client/suites/http_route/filters/security.rs`. The latter
  accepts matching absolute-form Host values with a port, routes a valid IPv6
  absolute-form authority to the normal no-route result, and rejects Host
  conflicts, userinfo, and malformed request-line separators.
- The `HTTPRoute/Transport` integration suite observes two valid requests on
  one HTTP/1 connection, then a duplicate-Host 400 and connection close;
  its origin recorder sees neither that rejected request nor a refused
  CONNECT. The same check runs with HTTP/1 and HTTP/2 origins.
- The dynamic `DuplicateHostPolicyHotReload` scenario now checks that rejection
  persists across an unrelated Gateway configuration update. It does not
  establish that an opt-out exists.
- A single `Host: a, b` field is not a duplicate field. Route matching or
  authority validation may reject it for a different reason; do not infer
  protocol acceptance from the duplicate-field rule alone.

Historical evidence: `http1-01` introduced the Gateway check on 2026-05-17;
`D0013` added the former policy hot-reload scenario on 2026-09-21. Those
results applied to earlier code and are not fresh verification of the current
fork. `P090-003` revised ownership after the 0.9.0 fork adoption.

## Re-evaluation triggers

- Pingora's authority validation or downstream-session policy changes.
- A new HTTP/3 ingress path is added.
- A regression shows duplicate Host fields reaching Gateway filters or an
  upstream on a supported listener.
