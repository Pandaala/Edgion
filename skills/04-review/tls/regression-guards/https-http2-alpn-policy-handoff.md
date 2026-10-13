---
name: https-http2-alpn-policy-handoff
description: Use when reviewing HTTPS listener HTTP/2 annotation handoff and the shared TLS settings factory's ALPN ownership.
knowledge_type: regression-guards
---

# HTTPS frontend HTTP/2 policy reaches its ALPN owner

## Conclusion and scope

The HTTPS branch of `add_http_listener` passes its resolved `enable_http2` flag
to `TlsCallback::new_tls_settings_with_callback`. The factory is the sole owner
that enables HTTP ALPN. Default and explicit enabled listeners negotiate h2
when a client offers it, with HTTP/1.1 fallback. Explicitly disabled listeners
leave ALPN unset and accept HTTP/1.1. The shared factory's false branch must not
install HTTP/1.1 ALPN: TLS-to-TCP also passes false and advertises no HTTP ALPN.

This is listener-startup transport policy. Annotation changes require restart;
it creates no dynamic listener-transport update contract. Plaintext h2c uses its
existing separate `HttpServerOptions` field. Native gRPC retains its
`Http2Required` check; HTTP/1.1 gRPC-Web remains independent of this flag.

## Rationale and evidence

ARCH-040 identified a literal true argument in the HTTPS builder. The factory
immediately installed h2 ALPN, and the later conditional caller `enable_h2`
could not undo it. Replacing the constant with the already resolved boolean
and removing that redundant call fixes the policy handoff without another
settings owner or a new configuration mechanism.

Authority is the [annotation contract](../../../02-features/05-annotations/references/annotations-metadata.md)
and [downstream TLS architecture](../../../01-architecture/02-gateway/04-tls/01-downstream-tls.md).
The existing annotation parser treats absence as enabled and case-insensitive
false/0/no/off as disabled; arbitrary present values remain enabled. No trimming
or validation behavior changes. The supported locked BoringSSL Pingora settings
start with no ALPN and `enable_h2` installs h2 with HTTP/1.1 fallback.

## Regression boundaries

The pure `parse_enable_http2_annotation_preserves_frontend_policy` test pins
absence, falsy tokens and case variants, enabling values and existing whitespace
behavior. The shared `EdgionTls/https` suite adds default/enabled/disabled
ordinary single-listener fixtures and negotiating-client response-version
assertions plus usable forced-H1 responses on all three. Enabled negotiation
proves the normal client actually offers h2; a factory-only regression would
miss the original wrong caller argument.

The separate Unix+BoringSSL `listener_tls_tcp_alpn` integration target calls the
actual existing TLS-to-TCP builder and performs a real BoringSSL handshake
offering h2 and HTTP/1.1, requiring no selected ALPN. It owns one in-process
Pingora server with bounded cleanup. Controller admission currently rejects
TLS/Terminate, so this narrow transport-seam test makes no Controller support or
TLSRoute conformance claim. It does not run alongside the shared topology.

No certificate/mTLS callback, handshake offload, H2 limit, request-body,
observability or cross-cutting owner is changed. The six new resource fixtures
are mirrored in the Kubernetes tree because the HTTPS suite is shared. The
Gateway Service exposes both new TCP ports. Parsed fixture equality, canonical
listener-port mappings and unique Service names/ports passed static checks;
no live Kubernetes execution or conformance is claimed. FIPS and unsupported
Gateway rustls are outside scope.

## Verification and re-evaluation

Fresh local verification on 2026-10-13 used the supported default BoringSSL
build and newly built Gateway, Controller, CLI, test client/server and cache
validator binaries:

- `cargo test -p edgion-gateway --lib` with focused filters
  `parse_enable_http2_annotation`, `downstream_h2`, `tls_offload`,
  `pg_init_downstream_modules`, and `pg_early_request_filter`: respectively
  1, 4, 3, 2 and 17 tests passed. This includes the native-gRPC frontend gate
  and independent gRPC-Web module/filter sequencing.
- `cargo test -p edgion-gateway --test listener_tls_tcp_alpn`: 1 test passed;
  the actual builder's BoringSSL handshake offered h2/HTTP1.1 and selected no
  ALPN. Canonical certificate compilation, listener indexing and connection
  admission were asserted before the handshake; bounded cleanup emptied its
  TLS/Gateway/Class inputs. Fixture setup initially omitted resolved Secret
  metadata and serving Gateway publication/status; correcting those inputs
  preserved the production fail-closed gates.
- `run_integration.sh --no-prepare -r EdgionTls -i https`: 9/9 cases passed
  separately with `EDGION_TEST_DOWNSTREAM_TLS_OFFLOAD=0` and `=1`. The enabled
  copied startup YAML contained the 1x1 offload pool settings. Both runs proved
  default/true h2 negotiation, false H1 fallback with h2 offered, and usable
  forced H1 on all three HTTPS listeners.
- The existing `HTTPRoute -i Protocol/H2C` suite passed 1/1; its assertion is
  backend/upstream H2C behavior, not a dedicated downstream enable/disable wire
  matrix. The plaintext h2c builder branch is structurally unchanged.
- `HTTPRoute -i Protocol/GrpcWeb` passed 4/4 on the enabled and HTTP/1-only
  Gateways. Existing `EdgionTls -i mTLS` and `-i grpctls` selections also passed
  with offload enabled. Every shared runner completed its preflight and final
  metrics, unknown-field, requeue and cache-residue gates with clean teardown.

Commands above use `edgion-tests/integration/scripts/integration/run_integration.sh`.
Complete command logs are retained in `impl/architecture-ARCH-040/`; shared run
reports are under `tmp/integration_testing/testing_20261013_{164950,165020,165046,165112,165246,165318}/`.
The initial shared attempt stopped at English-only preflight for an approval
quote in the coordinator record; translating that record fixed it before any
services were started. This was not a runtime failure. No full workspace test
suite, live Kubernetes, FIPS build or performance measurement was run.

Re-evaluate when the factory's false/default ALPN semantics, the startup
listener carrier, the annotation parser, supported Pingora revision, or native
gRPC/gRPC-Web classification changes.

## Resolution references

- ARCH-040: HTTPS HTTP/2 annotation policy handoff; implementation and fresh
  verification tracked under `impl/architecture-ARCH-040/`.
