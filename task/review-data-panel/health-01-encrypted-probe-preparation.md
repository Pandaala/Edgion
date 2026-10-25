# Encrypted health-probe preparation

Status: deferred for user decision. No runtime change.
Source baseline: c0881667b.

## Evidence and scope

The health manager constructs EncryptedProbeTransport once for its policy-owned
probe task. Its shared verified TLS configuration already avoids rebuilding CA,
SAN and client-identity policy for every exchange. Remaining preparation includes:

- HTTPS clones ClientConfig, creates an endpoint-pinned resolver and builds a
  fresh canonical outbound client for each exchange. Proxy pickup is disabled,
  redirects are denied and idle pooling is disabled.
- GRPCS clones ClientConfig, installs h2 ALPN, creates a connector and parses an
  owned ServerName for each exchange. It then creates an endpoint-specific tonic
  channel with the HTTPS origin and the existing deadlines.

These are source-level costs, not a demonstrated bottleneck. In particular,
ClientConfig cloning does not imply fresh TLS session state: locked rustls
0.23.45 clones an Arc-backed Resumption store. Any proposed sharing must inspect
actual dependency ownership rather than infer isolation from a fresh wrapper.

## Decision boundary

Keep the current implementation, or authorize a bounded design for preparing
protocol-specific immutable TLS/ServerName data once per task generation while
retaining exchange-local clients and channels. No alternative is selected here.
Account for retained configuration memory and preparation paid by unused or
immediately cancelled tasks, as well as repeated work avoided.

Client/channel pooling is a different lifecycle change and is outside this
proposal. The current architecture requires each endpoint exchange to own its
client/channel. Shared URL-authority pools cannot be assumed to honor a changed
endpoint resolver or TLS generation. A broader pooling proposal would need
separate authorization and review of that ownership contract.

HTTPS also crosses the shared OutboundClientConfig API, whose preconfigured_tls
field currently owns ClientConfig. Do not expand that common-crate API or create
a health-specific TLS/client factory as an incidental local optimization.
Precomputing ServerName may move validation and failure timing; verify exact
errors and ordering instead of treating earlier validation as automatically safe.

## Acceptance before implementation

Use IMPL with an exact allow-list and independent ownership/value review. Preserve
health-local TLS and its independence from BackendTLSPolicy and global defaults;
CA, expiry, hostname, additional SAN and mTLS checks; numeric-host rejection;
exact endpoint pinning and dial-time safety checks; h2 ALPN and HTTPS origin;
proxy/redirect restrictions; deadlines and cancellation. Preserve task-generation
admission, stale completion rejection and certificate-rotation behavior. Do not
claim revocation of already-submitted transport I/O.

Verify concurrent probes for distinct endpoints sharing a TLS hostname, generation
replacement while work is queued/in flight, malformed material and names, timeout
and cancellation cleanup. Record configuration/session-store sharing and retained
memory explicitly. Measure preparation cost before claiming latency or throughput
improvement; existing behavior tests alone cannot establish a performance benefit.

## Sources and verification limits

- edgion-gateway/src/backends/health/check/encrypted.rs
- edgion-gateway/src/backends/health/check/manager.rs
- edgion-common/src/link_sys_core/http_client.rs
- skills/01-architecture/02-gateway/07-backends.md
- skills/04-review/backends/intentional-design/active-health-check-transport-is-independent.md
- Locked rustls 0.23.45 client/client_conn.rs: ClientConfig and Resumption.

The existing health-check selection passed 40 unit tests serially at this
checkpoint. No runtime/test changed, no alternative was prototyped, and no live
transport integration, full suite or performance benchmark ran. These tests
support current behavior, not acceptance of a new preparation or pooling design.
