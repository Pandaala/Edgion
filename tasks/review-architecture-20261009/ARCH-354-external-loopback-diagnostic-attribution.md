---
id: ARCH-354
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-472
analyst: /root/analyze_outbound
baseline: 11aeb2347657135da6388057bbf90a4f239e5c16b822a8132cc94916aed7718b
---
# Attribute external-loopback rejection to the current request diagnostic

`skip_matched_localhost_rejected` and `reject_matched_localhost_rejected` treat every received HTTP 500 or 502 as proof that external-loopback admission rejected the selected target. Neither correlates the request with its terminal diagnostic. If selection succeeds and localhost resolution succeeds, but the admission check is allowed or bypassed and the subsequent dial fails, the same 502 satisfies these assertions. DNS or other Gateway failures can also satisfy them. This is a source-level false-positive witness, not an executed security regression or a claim that the complete suite passes after bypassing protection.

The runtime already owns precise evidence. `DynamicExternalUpstream::run_request` publishes an authorized External jump and logs “upstream selected” before address admission. `pg_upstream_peer::try_external_jump_peer` resolves the domain, calls `validate_resolved_domain_addr`, records `BackendLocalhostNotAllowed` on rejection and retains the rejected address in the canonical upstream attempt. Therefore selection logs alone cannot prove the later security decision. Existing plugin unit tests verify selection, Host mutation order, dye outcome and jump rejection; they do not substitute for this live admission witness.

Both fixture trees select localhost targets. Their policy preconditions differ: the local base omits the loopback override, while the current Kubernetes base explicitly sets `securityProtect.allowLoopbackUpstream: true`. The original upstream-address-safety contract permits that override. A failed localhost dial in the Gateway Pod must not be described as loopback-policy enforcement, and changing the shared Kubernetes allowance globally would disturb unrelated accepted fixture ownership.

For these two rejection probes, retain the expected 500/502 outcome and require the exact current request's existing error diagnostic `BackendLocalhostNotAllowed`. Use a fresh marker, canonical `traceparent_for_marker`, `access_log: test_store`, and the existing authenticated AccessLogClient retry path. Missing, malformed, stale or unavailable evidence fails. Establish the deny-loopback precondition through an isolated suite-owned fixture/publication boundary; preserve the shared Kubernetes allow-loopback base. Until that precondition is established, report the incompatible test configuration rather than passing an ordinary dial error as security evidence. No production policy, error mapping, DNS behavior, metric or trace log changes are needed.

A positive fallback request is useful readiness control but cannot attribute the later failed exchange. Narrowing to 502 alone also cannot distinguish connection failure from rejection. This is the external DNS admission boundary, distinct from ARCH352's upstream certificate-SAN hook. Host-override and dye-label assertions have separate coverage limits and are not folded into this finding.

Future verification should pass genuine loopback rejection with the matching fresh diagnostic, fail unrelated 500/502 or allowed-loopback dial failures, and preserve fallback/reject/regex cases, canonical listener selection and both fixture topologies. All primary implementations, actual producer/client seams, real unit bodies and original decisions were inspected; no execution or mutation occurred.
