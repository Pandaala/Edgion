---
name: forward-auth-webhook-ssrf
description: Decision rule for ForwardAuth / Webhook SSRF findings. Read this file first when reviewing "target lacks internal IP filtering, can probe metadata service" or "uri field lacks internal IP filtering" findings — covers both the Service-backendRef target (NOT-AN-ISSUE, RBAC/NetworkPolicy is the correct defense) and the URL target (has its own EgressResolver + blockPrivate guard; do not ask for more).
knowledge_type: regression-guards
---

# ForwardAuth / Webhook SSRF — triage rule

`WebhookTarget` (shared by the LinkSys Webhook resource and the `forward_auth` plugin, via
`ExternalHttpConnection`) now has **two** target forms, each with its own SSRF story. Identify
which form the finding is about before applying a verdict.

```yaml
# Service target — RBAC + NetworkPolicy is the defense (see §1)
target: { name: auth-svc, namespace: production, port: 8080 }

# URL target — has its own code-layer guard (see §2)
target: { url: https://api.example.com, blockPrivate: true }
```

## 1. Service target — NOT-AN-ISSUE (for additional block-list suggestions)

For a Service-backed target, the lack of additional internal IP filtering **does not constitute a
fixable security issue beyond what is already implemented**; suggestions of the form "add a
private-IP block list" or "add an SSRF protection library" are not accepted.

### What the guard already does

1. The gateway resolves the Service to pod endpoints via EndpointSlice.
2. `ManagedOutboundClient::dial_url` calls `extract_ip_literal` on the resolved URL.
3. If the host is an IP literal, `validate_endpoint_backend_addr` is called with `allow_loopback = false`.
4. The guard rejects loopback (`127.x.x.x` / `::1`) and link-local (`169.254.x.x`) addresses,
   propagating as `ManagedOutboundDialError::SsrfRejected`.

Source: `edgion-gateway/src/plugins/http/common/outbound/client.rs` — `dial_url` + `extract_ip_literal`.

Cluster-local DNS names (e.g. `auth-svc.production.svc.cluster.local`) still bypass this
literal-level guard for Service targets — `extract_ip_literal` only classifies IP literals, and a
Service's resolved pod IPs are deliberately not run through the stricter egress policy described
below (RFC1918 must stay allowed; see §1.1). **This is unaffected by the URL-target guard in §2**,
which is a separate code path with its own resolver.

### 1.1 Why Service targets keep the permissive stance

RFC 1918 private addresses (`10/8`, `172.16/12`, `192.168/16`) are the normal destination for
in-cluster auth/webhook services and cannot be blocked without breaking every legitimate
deployment. `169.254.0.0/16` (cloud IMDS) is already blocked by `validate_endpoint_backend_addr`.
Adding more CIDR blocks here:

- Would break legitimate in-cluster auth/webhook deployments.
- Cannot defend against DNS rebinding or hostname forms (e.g. `metadata.google.internal`) — a
  Service's cluster-local DNS name is never itself classified as loopback/link-local.
- Is not cost-effective enough to justify the breaking change.

**Threat model requires a high-privilege precondition.** Exploiting this path requires:
- ForwardAuth: write permission to HTTPRoute or EdgionPlugins CRDs (namespace-admin level).
- Webhook: write permission to the namespaced LinkSys CRD. Its omitted Service
  target namespace inherits the actual LinkSys namespace; this does not imply
  cluster-scoped resource ownership or automatically grant cluster-wide authority.

An attacker with these permissions can already do far more harmful things (reroute traffic, inject
backends, read Secrets, etc.). SSRF via a Service target is not a boundary breakthrough but lateral
movement inside an already-broken boundary. RBAC is the correct defense; expanding the IP block
list for Service targets is not.

**Industry comparison.** Traefik `forwardAuth`, Envoy `ext_authz`, APISIX `forward-auth` all do not
perform URI-layer SSRF filtering on their Service/upstream-style targets beyond what network policy
provides, and consistently rely on RBAC + network policy.

### The correct defense layers for a Service target

- **Existing code guard**: `validate_endpoint_backend_addr` blocks loopback and link-local IPs on
  resolved pod addresses (forward_auth + any future `ManagedOutboundClient` Service-target consumer).
- **K8s NetworkPolicy**: limit which IPs/ports the gateway pod may reach.
- **RBAC**: control who may write HTTPRoute, EdgionPlugins, and LinkSys CRDs.
- **Cloud side**: enable IMDSv2 (requires a PUT token; direct GET returns 401).

### Fix suggestions not accepted (Service target)

- Add a private CIDR block list for Service targets (breaks legitimate in-cluster auth/webhook
  services; RFC 1918 addresses are valid pod IPs).
- Introduce a generic `ssrf-guard` library for Service targets (overkill, cannot solve DNS rebinding).
- Block `169.254.x.x` separately (already blocked by `validate_endpoint_backend_addr`).
- Require a Service `target` to be DNS-resolved and validated at admission time (introduces
  startup-time DNS dependency and race conditions).
- Change a Service `target` into a free-form URI with IP validation as the *only* addressing form
  (would remove EndpointSlice-based load balancing and the existing loopback/link-local guard) —
  moot in any case, since the URL form now exists as an additional, not replacement, target kind
  (§2).

## 2. URL target — has its own guard; do not ask for more without reading §2.3 first

A URL target (`target.url`) is addressed by hostname, which the Service-target guard above cannot
protect (a hostname is never an IP literal). It has a **separate, purpose-built** guard: two
enforcement points implementing one policy.

1. **Hostname URL** → `EgressResolver` (`edgion-gateway/src/plugins/http/common/outbound/egress.rs`),
   a `reqwest::dns::Resolve` implementation, classifies every resolved IP *before* reqwest connects.
   reqwest does not re-resolve after `Resolve::resolve` returns, so there is no TOCTOU /
   DNS-rebinding window between classification and connect.
2. **IP-literal URL** (e.g. `target.url: http://10.0.0.1:8080`) → `hyper-util`'s connector skips DNS
   entirely for an already-literal host (`hyper-util` `src/client/legacy/connect/http.rs:538`), so
   `EgressResolver` is never invoked. `ManagedOutboundClient::dial_url` instead applies the same
   `EgressPolicy` to the literal directly (`client.rs`, the `egress_policy: Option<EgressPolicy>`
   field is `Some` only for URL targets).

Policy (both enforcement points implement the same one):

- Production URL targets reject loopback, link-local, unspecified and non-dialable
  addresses as well as configured blocked CIDRs. Private-address blocking additionally
  rejects RFC1918, shared IPv4 space and IPv6 unique-local addresses.
- The gateway-local default for `blockPrivate` when a resource omits it is
  `outbound_egress_block_private` / `EDGION_OUTBOUND_BLOCK_PRIVATE` (default `false`); an explicit
  `target.blockPrivate` on the resource always wins over that default.
- A URL target always gets its own dedicated `reqwest::Client` (never the process-global shared
  client) and `EgressResolver`. Resolved answers live raw in the process-wide egress
  `DnsAnswerCache` (keyed by resolver, name and query policy) and each resolver classifies them with
  its own policy on every use, so sharing an answer never shares an egress decision (common-02,
  [DNS answer cache identity and lifecycle](../../backends/regression-guards/dns-answer-cache-identity-and-lifecycle.md)).

The egress resolver counts each rejected DNS answer. For literals, the URL policy
counts private and unspecified destinations. The earlier backend admission check
rejects loopback, link-local, non-dialable addresses and configured CIDRs before
the URL metric is emitted. See the [connector egress policy](../../../../docs/en/ops-guide/infrastructure/connectors.md#egress-policy-ssrf-protection)
for the metric reasons and configuration.

A test (`edgion-gateway/src/plugins/http/common/outbound/egress.rs` unit tests, plus the client-level
literal tests) pins that the hostname path and the IP-literal path agree, since they are reached by
disjoint code and could otherwise drift silently.

**This is still defense-in-depth, not a replacement for network controls.** The primary egress
control for any outbound call — Service or URL target — remains a NetworkPolicy on the Gateway Pod
(`edgion-gateway/src/util/net.rs:104`). `EgressResolver` / `blockPrivate` exist because a URL target
lets an operator (with LinkSys/EdgionPlugins write access) point the gateway at an arbitrary
external host by hostname, which a NetworkPolicy alone cannot distinguish from a legitimate SaaS
endpoint at request-authoring time — the code-layer guard narrows what that already-privileged
write can reach.

### 2.1 What a URL-target finding should NOT ask for

Given the guard above already exists, a URL-target SSRF finding should only be raised if it
identifies a **concrete gap** in §2's mechanism — e.g. a code path that builds a URL-target client
without going through `EgressResolver`/`egress_policy`, or a policy mismatch between the hostname and
IP-literal enforcement points. Generic suggestions already covered by the existing mechanism are not
new findings:

- "Add a private CIDR block list for URL targets" — already exists as `blockPrivate` /
  `outbound_egress_block_private`. Do not re-propose it; if it is disabled by default, that default is
  a recorded design decision (§1.1's rationale extended to URL targets), not an oversight.
  A finding that the *default* should flip to `true` is a policy debate, not a "missing guard"
  finding — file it as such if you disagree, but do not describe `blockPrivate` as absent.
  Cluster administrators who want the stricter posture flip the global default; individual webhooks
  that need an internal artifact repository on `10.x` opt out per resource.
- "DNS names bypass the IP guard" — true for **Service** targets (§1), not for **URL** targets: a
  URL target's hostname is resolved and classified by `EgressResolver` at connect time, with no
  race, because reqwest does not re-resolve after `Resolve::resolve` returns.

### 2.2 History: why hostnames were unguarded before this feature

Before the URL-target work landed, this file claimed hostnames were safely delegated: "if the host
is a numeric IP address, `validate_endpoint_backend_addr` is called" — implying a non-numeric host
was handled some other safe way. That was false. The guard used
`pingora_core::protocols::l4::socket::SocketAddr::from_str`, whose Unix implementation
(pingora-core 0.8.1, `src/protocols/l4/socket.rs:205`) falls back to
`StdUnixSockAddr::from_pathname(s)` when IP parsing fails. A hostname such as
`"api.example.com:443"` is a valid pathname, so it parsed successfully into
`SocketAddr::Unix(..)`, and `classify_backend_addr`'s `addr.as_inet()?` short-circuited to "allowed"
for every non-IP-literal target — the guard was a no-op for hostnames, not an intentional bypass.

The fix (part of the URL-target work) renamed the function to `extract_ip_literal` and switched it
to `std::net::SocketAddr::from_str`, which returns `None` for a non-IP-literal string instead of
silently manufacturing a `Unix` address. `None` is now an explicit, intentional skip (documented at
the call site) rather than an accident of pingora's UDS fallback — and it also removes the spurious
"Raw Unix domain socket path support will be deprecated" warning that used to fire on every
`ServiceClusterIp` dial.

For a **Service** target, `None` still means "skip the literal-level guard" (§1 — the DNS name is a
cluster-local Service address, and cluster-local RFC1918/DNS is deliberately not blocked here). For
a **URL** target, `None` instead routes to `EgressResolver` (§2), which is the guard that closes the
gap this history section describes. Do not re-file "the guard is a no-op for hostnames" against a
URL target — that was the pre-fix state, not the current one.
