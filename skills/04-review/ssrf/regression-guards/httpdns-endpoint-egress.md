---
name: httpdns-endpoint-egress
description: Review HTTP DNS endpoint dialing, bootstrap resolution, and per-LinkSys loopback exceptions.
knowledge_type: regression-guards
---

# HTTP DNS endpoint egress

## Conclusion and scope

The HTTP DNS service address is a separate egress boundary from the backend
addresses returned in its response. Both Controller background resolution and
Gateway request-time resolution use Common's HTTP DNS fetcher.

`HttpDnsConnection.allow_loopback` is the operator-visible
`connection.allowLoopback` boolean, default false. Explicit true permits loopback
for this resolver only; RFC1918 and IPv6 ULA services are always allowed.
Link-local and unspecified addresses remain denied, including mapped IPv4.
The shared explicit integration-testing-mode exception also permits loopback;
it is not a production deployment option.

## Rationale and evidence

Canonical authority: [unification rules](../../../00-design-rules.md#d-cross-cutting-unification--one-canonical-implementation-per-mechanism).
`edgion-common/src/dns/http_fetcher.rs` installs Common `EgressResolver` with
fixed `ResolverSpec::System` bootstrap. The connector consumes the admitted
addresses directly, avoiding a check-then-resolve DNS rebinding gap and recursive
HTTP DNS bootstrap. Literal hosts bypass reqwest DNS, so the fetcher classifies
the normalized host before sending the same parsed URL.

The shared client factory still owns TLS, timeout, disabled proxy environment,
and denied redirects. Endpoint rejection flows through the existing configured
fallback; it does not change backend address policy. The new boolean remains
serialized for conf_sync and is included in the resolver cache fingerprint.
LinkSys config uses CRD preserve-unknown-fields; the typed schema exposes the
boolean and serde rejects non-boolean values.

## Alternatives and limits

The user explicitly chose default false with a per-resolver override for local
sidecars; no generic configurable deny-list was requested. Private-IP denial
would break legitimate internal DNS services. Loopback in different Controller
and Gateway Pods refers to different local services.

This closes an operator-configured egress coverage gap, not a demonstrated
unauthenticated arbitrary-URL attack. A permitted local proxy can itself forward
requests elsewhere; its own destination policy is outside this guard.
System/standard DNS server addresses and returned backend policy are separate.
Client/TLS preparation now belongs to configuration generations rather than
individual queries. See
[prepared HTTPDNS generations](../../link-sys/regression-guards/httpdns-prepared-runtime.md)
for publication, fallback, and certificate-rotation boundaries.

## Verification and re-evaluation

Production-build transport tests in `edgion-common/tests/httpdns_fetcher.rs`
cover default literal/hostname loopback denial, normalized/mapped literals,
explicit allowance and revocation, metadata/unspecified denial, fallback,
redirects, timeout, and response bounds. Resource tests cover typed schema and
LinkSys transport; Common tests cover cache fingerprint changes.
The DnsResolutionFlap scenario adds a blocked endpoint update and recovery;
integration-testing-mode cannot prove production loopback denial.

Verified in this fix:

- `cargo test -p edgion-common --test httpdns_fetcher`: 8 passed.
- `cargo test -p edgion-resources -p edgion-common --lib httpdns`: 45 passed.
- `cargo test -p edgion-common --lib egress::tests`: 8 passed, including private/ULA acceptance.
- `./cicd/checks/check_crd_coverage.sh`: 27 passed across both targets.
- `test_client -g --dynamic-scenario DnsResolutionFlap`: passed against freshly
  built binaries in an owned local Controller/Gateway/test-server topology,
  with no service restart during configuration transitions. K8s fixtures were
  updated and statically validated, but no Kubernetes cluster run was performed.
- Workspace all-target check/clippy, formatting, agent-doc, SSA, metrics,
  Pingora source, Gateway API transform, and unit-test-layout guards passed.

The temporary topology runner needed proxy bypass, CRD staging, and canonical
filesystem resource filenames before the successful run; those setup failures
were not HTTP DNS policy failures. Revisit when bootstrap, redirects, proxy
policy, endpoint preparation, cache identity, or address classification changes.

## Resolution references

- `define-httpdns-endpoint-egress-policy` (Common batch B0083): real endpoint
  egress gap; per-LinkSys default-false loopback policy selected by the user.
