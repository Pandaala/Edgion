---
name: egc-runtime-snapshot-publication
description: Review EGC hot updates, shared runtime publication, request and connection capture, and missing-reference defaults.
knowledge_type: regression-guards
---

# EGC runtime snapshot publication and lifecycle capture

## Conclusion and scope

Edgion propagates updates to a listener's referenced `EdgionGatewayConfig` into
new runtime consumers without rebuilding the listener. The user explicitly
authorized full EGC dynamic configuration with one capture per request or
connection when resolving `audit-security-protect-egc-hot-reload` (B0066/D0012).
This is Edgion's product contract, not a general Kubernetes requirement.

One existing `ArcSwap<EdgionGatewayConfigSnapshot>` publishes the resource map and
`HashMap<String, Arc<ResolvedGatewayConfig>>` together. At construction, listeners bind their selected name to a shared
`Arc<GatewayConfigHandle>` containing an `ArcSwap<ResolvedGatewayConfig>`.
New requests/connections load directly from that handle and retain the Arc. Config parsing,
RealIp validation and matcher construction happen during publication. Consumers
do not search the EGC table or construct configuration on the hot path.

## Rationale and evidence

The original defect was that updated EGC resources entered the store while
HTTP, TCP, UDP and TLS consumers continued reading startup-owned configuration.
It affected `xForwardedForLimit`, `requireSniHostMatch`, `allowLoopbackUpstream`,
`tlsProxyLogRecord` and `fallbackSni`. The broader audit also found startup-bound
HTTP timeouts, retries, RealIp, compression and keepalive configuration.

`config/edgion_gateway/conf_handler_impl.rs::ResolvedGatewayConfig::resolve`
compiles those fields alongside existing body, forwarded-header, preflight and
L4 timeout policies. `bind_gateway_config` and publication share the writer lock,
so binding concurrently with an update cannot miss the new generation.
`GatewayConfigHandle::load` clones the per-EGC Arc without a lookup or lock.
Full sets, partial updates and removal use the same publication. Missing names
bind before resource arrival; deletion stores defaults into the existing handle,
and same-name recreation updates that handle again.

| Consumer | Capture boundary and retained meaning |
|---|---|
| HTTP `proxy_http/pg_new_ctx.rs` | One request generation for security, RealIp, timeouts/retries, body, headers, preflight and compression |
| HTTP/1 persistent connection context | First request captures the keepalive budget; subsequent requests retain that budget, with existing reuse-count semantics |
| `routes/tcp/edgion_tcp.rs`, `routes/tls/{proxy,passthrough_proxy}.rs` | `process_new` captures timeouts, loopback permission and TLS proxy logging through retries and relay |
| `routes/udp/edgion_udp.rs` | New session captures policy; existing-session datagrams do not reload EGC |
| `tls/runtime/gateway/tls_pingora.rs` | Handshake callback captures fallback SNI for certificate selection |

No reference is distinct from the real name `default`. Missing/deleted references
use a shared runtime default Arc. In particular, loopback remains denied, SNI/Host
protection defaults on, RealIp extraction is disabled, fallback
SNI is absent, and L4 timeouts retain their established defaults. XFF has an
existing distinction: absent `securityProtect` means no byte cap, while a present
block with an omitted field defaults to 200. Exceeding the cap returns HTTP 400;
it never truncates XFF.

The request metadata snapshot counts repeated XFF fields in order, including the
two-byte separator between empty fields. It returns no value immediately when
checked aggregate length overflows or exceeds the captured limit. Invalid text
within the limit still requires counting later fields: a later limit crossing
must retain the over-limit result. Exact-limit values remain accepted, and no
limit still means unbounded aggregate length subject to checked arithmetic.
Single-field snapshots reuse their first text validation. Host and path error
precedence remains owned by metadata validation, not by this snapshot helper.
The XFF snapshot boundary fixtures cover these cases; early rejection reduces
remaining field scanning without asserting a measured throughput improvement.

## Alternatives and limits

There is no polling worker or duplicate resolved configuration. A weak binding
registry under the writer lock preserves live handle identity. Named binding and
publication prune expired entries; a last dropped handle leaves only its weak
entry/key until the next such event. It retains no runtime generation.
`egc_bindings` in store-stats exposes count, keys and expired count. Total count
is a structural residue exemption because startup-owned listeners survive resource
wipe; expired entries remain checked and must be pruned by publication.
Handle publication is atomic per EGC, not across all handles, the raw/admin
snapshot, DNS or plugin publishers. In-flight consumers keep their
captured generation; a completed TLS handshake does not reselect its certificate.
TCP/TLS proxy capture occurs at application processing, not necessarily raw socket
accept. Keepalive capture occurs on the first HTTP/1 request, not socket accept.

Global LinkSys, load-balancing defaults, outbound TLS and DNS remain dynamic through
their existing consumers. Plugin policy/global plugins/access-log masking retain
their scoped publication. These mechanisms are not one transaction with the HTTP
runtime Arc. Missing referenced plugin policy still denies admission independently
of runtime-default fallback; see [scoped policy publication](../../plugin-runtime/regression-guards/scoped-policy-runtime-publication.md).

This change does not retarget GatewayClass `parametersRef`, transfer a shared
physical listener's ownership, rebind ports, or support Gateway deletion/recreation.
Response compression retains the [known-length restriction](../../http1/compatibility-constraints/compression-requires-known-response-length.md).
Request-limit versus Pingora reuse-budget accounting is outside this publication
rule's verification scope. Its corrected total-request semantics and wire coverage
are maintained by the [HTTP/1 keepalive counting guard](../../http1/regression-guards/downstream-keepalive-request-limit-counting.md).

## Verification and re-evaluation

The publication unit test
`runtime_egc_hot_reload_pins_security_and_compiled_http_policy` covers old/new
runtime Arcs, security values, compiled HTTP policy and missing/deleted defaults.
The request-context test
`request_context_pins_preflight_policy_from_one_snapshot_generation` covers
consumer capture. `EgcRuntimeHotReload` provides the dynamic HTTP/TLS scenario;
`L4TimeoutHotReload` retains the separate relay timeout boundary.

Verification before the stable-handle optimization (2026-09-21): all 33 EGC config unit tests and 470 HTTP unit
tests passed. The real-socket `tls_passthrough_clienthello` regression passed,
including loopback denial for new connections, changed logging and old-connection
bidirectional traffic with its captured logging policy. `EgcRuntimeHotReload`
passed XFF/compression/fallback-SNI update, removal, restoration and old-connection
checks. `L4TimeoutHotReload` passed TCP, TLS termination and TLS passthrough.
The direct route fixture supplies the current Controller-resolved status owner;
the shared topology used freshly built Controller and Gateway binaries.
Kubernetes fixtures and scenario registration were validated statically; no live
Kubernetes run or full `cargo test --all` was performed. Historical L4 timeout
results remain in their [owning rule](l4-timeout-snapshot-publication.md).
Independent review, workspace check, workspace Clippy, formatting and the
repository documentation, SSA, metrics, Pingora-source and Gateway API contract
guards passed. Clippy retained two unrelated pre-existing test warnings.

Stable-handle verification (2026-09-21): 35 EGC configuration tests, 470 HTTP
tests, 46 UDP tests, the TLS passthrough socket regression and the cache-residue
validator regression passed (553 targeted tests). Both `EgcRuntimeHotReload`
and `L4TimeoutHotReload` passed on the rebuilt Gateway. Independent review
verified the implementation and these logs. The added lifecycle test covers
binding before arrival, shared identity, removal/recreation, full/partial updates,
old-generation release and expired-entry pruning. The concurrent bind/publication
test verifies no update is lost; the residue test verifies expired entries remain
detectable while live listener bindings are exempt. No throughput benchmark was
run: the demonstrated performance change is removal of request-side name hashing
and resource-table lookup, not a measured throughput percentage.
Workspace check, workspace Clippy, formatting and all five repository guards
listed above also passed for the stable-handle change; Clippy reported only the
same two unrelated existing test warnings. No live Kubernetes run or full test
suite was performed for this optimization.

Re-evaluate when a new EGC field or consumer is added, a field needs immediate
revocation of in-flight work, listener ownership becomes dynamic, Pingora module
ordering changes, or shared-subsystem publication gains stronger atomicity.

## Resolution references

- `audit-security-protect-egc-hot-reload` — B0066/D0012: startup-frozen security
  values corrected through shared EGC runtime publication; user expanded scope to
  the remaining EGC runtime consumers.

### ARCH-028 RealIp reference ownership

HTTP RealIp and global EGC RealIp store only nonempty `trustedIpRefs`; IpGroups live in referenced IpLists. The actual owning EP/EGC catalogue provides the same cold key compiler as Boolean IP conditions. A global EGC requires explicit target namespaces and permits only current Cluster visibility or the controller's validation-disabled scope; its TLS administrative exemption and namespaced grants do not apply. Controller generation processing clears old proof and dependency edges before recomputing from actual fields, including missing-target recovery edges. Shared EGC Accepted remains intrinsic-only; deployment-local trust availability adds no shared condition.

The global policy captures a ConfigData snapshot directly before PluginSession exists; the HTTP policy reuses its dispatch-local adapter view. Both preflight every trust ref once and use that same view for the PROXY-effective downstream peer and all forwarded hops. Any unavailable list makes all trust false for that decision; content/visibility/delete/recovery changes remain live without rebuilding policy or copying CIDRs. Invalid input publishes a sealed plugin generation or disables global extraction, with clean-generation recovery. The narrow realIp CRD preservation and typed invalid marker prevent removed/unknown fields from becoming silently pruned successful configs; resolver-only proof is schema-hidden and recomputed on ingest. Referenced IpLists retain central partial-good entry compilation, replacing the former inline strict-trust admission.
