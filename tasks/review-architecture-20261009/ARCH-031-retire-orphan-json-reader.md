---
id: ARCH-031
status: candidate
kind: architecture-debt
priority: P3
primary_partition: PART-295
independent_verification: confirmed by /root/verify_ip_union
---

# Retire the unused JSON reader and test the current bounded OIDC path

## Current ownership and evidence

http_util.rs exposes fetch_json_limited as a shared OIDC/OAuth/JWKS JSON reader
and retains its own streaming accumulation loop plus formatted String
transport/parser errors. The module remains publicly exported by common/mod.rs:21.
A workspace-wide Rust caller search finds exactly four calls, all in
tests/unit/plugins/http/common/http_util_test.rs; no production caller remains.

The actual OIDC reader is openid_connect/error.rs::read_oidc_json (lines 8–25).
It applies the Content-Length check, composes
common/outbound/call.rs::stream_body_capped, rejects truncation before
deserialization, and maps errors to data-free OidcDependencyFailure
classifications. Discovery, JWKS, token exchange/refresh, introspection and
userinfo use that current reader. This finding does not allege unsafe production
OIDC accumulation or a second active production buffering engine.

## Concrete maintenance cost

All four retained legacy-helper tests are ordinary #[tokio::test] functions.
Their shared spawn_server_with_body binds a real Tokio TcpListener on 127.0.0.1,
spawns a server, and each test calls reqwest::get. They test the orphan helper's
bounded success, advertised-length rejection, chunked overflow and formatted
parse-error text instead of the real OIDC error contract. The original graded
rule X and unit-testing guide explicitly prohibit HTTP servers/network in unit tests.

A future response-cap or OIDC failure-policy change currently leaves a public
legacy entry point, another accumulator and four network-dependent assertions to
assess despite no serving consumer. These are concrete source/test ownership
costs; there is no observed failed CI run or runtime incident in this review.

## Bounded improvement and alternative

Retire the unused helper and its module export after a fresh caller audit.
Preserve the meaningful limit/streaming/parser assertions as focused network-free
unit tests of the current OIDC reader and canonical capped primitive. Synthetic
http::Response<reqwest::Body> values can exercise the real reader: locked reqwest
0.12.28 has an impl<T: Into<Body>> From<http::Response<T>> for Response at
async_impl/response.rs:456, and stream bodies can supply controlled chunks/errors
without a listener. Keep production visibility private and use the established
source-owned unit-test placement.

Track which old cases are replaced. Retired legacy String-message assertions
require explicit justification: those messages belong to an API with no
production consumers and should not be preserved as compatibility shims or
leaked into the active typed OIDC contract. Do not delete security/boundary
coverage merely to reduce a test count. Existing outbound transport tests
already cover exact/overflow/false-length/chunk errors, but test names alone are
not proof of complete replacement for the OIDC composition.

Keeping the helper in cfg(test) would eliminate the public production surface
but retain a separately tested algorithm and would not validate the actual reader.
Delegating it to the canonical accumulator would reduce duplication yet preserve
an unused formatted-error API and unnecessary module. Neither is needed unless
the fresh caller audit discovers a real consumer.

## Invariants, limits and future verification

Preserve current endpoint-specific caps, exact-limit acceptance, rejection before
parse on truncation, fixed ResponseTooLarge/ResponseRead/ResponseJson categories,
secret redaction, authentication versus dependency response/fallback policy, and
no main-response BodyStorage/aggregation. The change needs no new client, limiter,
cache, metric or request tracing.

Future verification must demonstrate network-free bounded-success, known
oversize, chunked overflow and malformed-JSON behavior through the current reader,
plus stream-error/cancellation behavior where applicable. Retain separately
authorized integration fixtures for real transport behavior. No tests or builds
were executed here.

Original decisions read: skills/04-review/oidc/regression-guards/authentication-response-byte-caps.md
and dependency-error-projection.md,
skills/04-review/testing/regression-guards/shared-outbound-call-unit-tests-are-network-free.md,
graded design rules X, and skills/05-testing/00-unit-testing.md section 5/untouchable
list. These records distinguish the typed OIDC composition from the old
formatted-error helper; none requires retaining an unused helper or ordinary
socket-based unit tests. This is distinct from ARCH-027's active Elasticsearch
acknowledgement receiving cap. Independent verification is pending.
