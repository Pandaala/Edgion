---
id: ARCH-272
status: confirmed
kind: behavior-defect
priority: P3
partition: PART-439
analyst: /root/analyze_outbound
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---
# Bind EdgionTls aggregate children to their own complete listener contexts

The supported `test_client -g -r EdgionTls` selection registers children for different TLS listeners under one ordinary HTTPS context. Some children therefore contact a different listener from the one their fixture and assertion are intended to exercise. This affects the direct aggregate binary selection; the current integration script's EdgionTls resource sweep invokes individual items and does not use this aggregate branch.

## Evidence and concrete behavior

`resolve_suite` accepts the resource-only selection as `EdgionTls`. `suite_to_port_key` maps that aggregate to `EdgionTls/https`. Main constructs one `TestContext` from that entry, selecting HTTPS port 31190, `test.example.com` as the HTTP/TLS host, and the CLI's default gRPC TLS port 18443 because the HTTPS entry has no `grpc_tls` field. The per-suite resolver is installed, but `add_suites_for_suite` registers the eight EdgionTls aggregate children directly, without their own listener contracts. These child implementations do not override `TestSuite::port_key`, whose default is `None`, so `TestRunner::run` clones the aggregate context for them instead of resolving their listener entries. See `edgion-tests/src/client/test_client.rs:3294` and `:3846`, and `framework.rs:777`.

`GrpcTlsTestSuite` constructs its endpoint from `ctx.target_host` and `ctx.grpc_https_port`. Its intended `EdgionTls/grpctls` entry and Gateway fixture use port 31200. The aggregate supplies 18443, whereas `test_client -g -r EdgionTls -i grpctls` selects 31200. This is an actual difference in the connection target, not merely redundant registration syntax.

The `PortOnlyEdgionTlsTestSuite` witness also demonstrates why binding only ports is insufficient. Its sole assertion requests `https://{ctx.http_host}:{ctx.https_port}/secure/health` and accepts a successful response containing the healthy/path marker. The targeted selection uses `port-only.example.com:31285`, matching the port-only Gateway and HTTPRoute fixtures. The aggregate instead supplies `test.example.com:31190`. The ordinary HTTPS fixture at that host and port exposes the same `/secure` prefix and forwards to the same test backend service. The port-only case can therefore report success based on an unrelated ordinary HTTPS listener, without exercising the port-only parent-reference binding. This is a source-supported false-positive opportunity for that case; it does not assert that the complete aggregate run passes, since other children can fail.

`SniCollisionTestSuite` similarly consumes the inherited HTTPS port instead of its own 31390 entry. These are manifestations of the same missing child-context ownership boundary, not separate findings.

The original `integration-port-selection-must-fail-closed.md` explicitly requires mixed aggregates to attach an exact key and protocol contract to every child. Its warning about direct-mode defaults is applicable to the gRPC TLS child. The current registration and context construction violate that requirement even though all relevant listener entries exist.

## Bounded improvement

Give each EdgionTls aggregate child the same complete effective context as its supported targeted invocation. Resolve the child's existing canonical `ports.json` key and required protocol before traffic, and carry the corresponding host/SNI choice and HTTP-client DNS override along with the ports. Keep this projection private to the harness selection boundary and reuse the existing static registrations, typed port resolver, context construction, and result reporting. The targeted and aggregate paths should consume one private context-selection implementation so a new listener or special host cannot require independent fixes in both paths.

Existing `with_listener` and `TestContext::with_ports` provide part of the mechanism, but `with_ports` only changes scalar ports and retains the old `http_host`, `grpc_host`, and HTTP client. Adding listener wrappers alone would leave the port-only host wrong. Rebuilding the child's existing test client when its effective host/DNS mapping changes is a cold harness operation; this proposal makes no allocation or throughput claim and does not alter production HTTP clients or TLS policy.

Preserve the eight existing aggregate child registrations, their order, suite hooks, case identities, assertions, fixtures, and failure/reporting semantics. A missing child mapping or required protocol must produce the existing infrastructure failure before that child's traffic. Do not substitute CLI defaults, weaken the port-only health assertion, add a runtime suite registry, or introduce a generic production factory for harness convenience. If aggregate EdgionTls selection is intentionally unsupported instead, rejecting that selection explicitly is a smaller alternative, but it removes currently accepted CLI behavior rather than restoring parity with targeted selections.

## Verification and limits

Verify selection without contacting listeners: compare effective ports, host/SNI inputs, and DNS mapping inputs for each aggregate child against its targeted invocation, including the gRPC TLS and port-only witnesses; verify missing mappings fail closed. Retain the current live integration cases and their identities as behavioral coverage. The source's embedded inventory tests, port-resolution tests, actual suite bodies, and fixture declarations were inspected, but no builds, tests, network calls, or mutations were performed.

The ordinary scripted EdgionTls resource sweep currently dispatches `-i https`, `-i grpctls`, `-i port_only`, and the other individual items. This finding does not claim that sweep currently chooses the wrong listeners, an observed CI failure, a production gateway defect, or measured performance impact. It addresses the accepted direct aggregate selection and the concrete loss of listener-specific assertion meaning.
