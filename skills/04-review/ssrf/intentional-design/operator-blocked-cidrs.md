---
name: operator-blocked-cidrs
description: Use when reviewing Gateway egress or backend address classification of cloud metadata endpoints outside link-local (Alibaba 100.100.100.200, AWS fd00:ec2::254, GCP fd20:ce::254, Oracle, Azure WireServer) or the outbound_egress_blocked_cidrs setting.
knowledge_type: intentional-design
---

# Operator-configured egress blocked CIDRs

## Conclusion and scope

Vendor cloud metadata and platform endpoints that sit outside link-local are
blocked through the Gateway-local, additive `gateway.outbound_egress_blocked_cidrs`
list, **not** through built-in classifier rules. The code default is an empty
list; the shipped configuration files (`config/edgion-gateway.yaml`,
`cicd/deploy/kubernetes/gateway/configmap.yaml`, `cicd/deploy/standalone/edgion-gateway.yaml`,
`cicd/deploy/standalone/packaging/config/edgion-gateway.yaml`) list Alibaba
`100.100.100.200/32`, AWS `fd00:ec2::254/128`, GCP `fd20:ce::254/128`, Oracle
`fd00:c1::a9fe:a9fe/128` and Azure WireServer `168.63.129.16/32`.

Current mechanism (`edgion-common/src/egress.rs`):

- The Gateway compiles the list at startup with `compile_blocked_cidrs`
  (`edgion_algo` `IpRadixMatcher`, `address/prefix` only; an invalid entry fails
  startup) and installs it once with `install_blocked_cidrs` before any outbound
  client or backend dial. The Controller never installs a list.
- `classify_ip` consults the installed list through `classify_ip_with`. Loopback
  and link-local keep their built-in classes, so their reasons and exceptions
  (`allowLoopbackUpstream`, integration-testing loopback) never change; any other
  covered address, IPv4-mapped forms included, becomes `IpClass::BlockedCidr`.
- `IpClass::BlockedCidr` is rejected under every policy: `egress_block_reason`
  (reason `blocked_cidr`, independent of `block_private` / `allow_loopback`),
  `classify_backend_addr` (`BlockedBackendReason::BlockedCidr` →
  `EdgionErrorCode::BackendAddrBlockedCidrNotAllowed`, log
  `unsafe blocked-cidr upstream`), DSL `is_ssrf_blocked_ip_literal`, GeoIP
  `classify_literal_destination`, Wasm OCI `guard_remote_target`, the HTTPDNS
  endpoint literal check, and `endpoint_slices::normalize_addrs` (DNS-derived
  endpoints drop matches before publication).

A re-report that a vendor metadata address outside link-local is reachable should
first check whether the deployment's config still carries the list entry. Demanding
that the address be hard-coded into `classify_builtin` contradicts this decision.

## Rationale and evidence

- User decision (2026-11-06, common-01): do not hard-code vendor addresses into
  the shared classifier. Deployments span many clouds, the vendor list can never
  be complete, and a vendor address can be a legitimate address elsewhere (a
  non-Alibaba cluster with a `100.64.0.0/10` pod CIDR may assign
  `100.100.100.200` to a pod); a hard-coded block would have no escape hatch.
- User decision (2026-11-12): keep the list additive-only and pre-populate the
  shipped configuration files with every known vendor endpoint, so a default
  deployment is protected while operators retain a config-file escape hatch.
- Classifying inside the shared core (rather than per call site) makes every
  existing consumer inherit the block, and the new enum variant turns each
  exhaustive consumer match into a compile-time decision point. The HTTPDNS
  endpoint check previously used a `_ => {}` wildcard; it is now exhaustive.
- Envoy/Istio delegate metadata protection to NetworkPolicy / IMDS hardening;
  dedicated SSRF filters (Smokescreen, `ssrf_filter`) ship explicit deny lists.
  A NetworkPolicy egress deny remains the documented primary control.
- Vendor addresses verified against vendor documentation on 2026-11-12: AWS and
  GCP official docs; Azure official docs for `168.63.129.16`; Alibaba documents
  only the IPv4 endpoint; the Oracle IPv6 address is taken from cloud-init's
  Oracle datasource (no Oracle document found).

## Alternatives and limits

- Rejected: built-in vendor ranges (no escape hatch, incomplete) and a code-level
  default list removable by operators (keeps vendor hard-coding in defaults; the
  shipped-config default achieves the same protection without it).
- Rejected: a per-call-site extra check (each new egress path could forget it).
- Accepted cost: a deployment whose own config file omits the key gets no vendor
  protection beyond link-local. Operators who replace the shipped config must
  carry the list forward.
- Accepted cost: the list cannot affect loopback or link-local; listing
  `127.0.0.0/8` does not override `allowLoopbackUpstream`.
- DSL applies the configured list to initial requests, DNS answers and redirect
  targets even with private-address blocking disabled. Wasm checks the parsed
  literal authority for callouts, URL sources and the initial OCI registry.
  Health probes check configured CIDRs for every endpoint source. The Gateway
  filters controller-origin DNS slices while constructing selectable endpoints.
- Kafka broker dials and Elasticsearch LinkSys do not use this classifier.
  L4 routes retain their protocol-specific unsafe-address statuses.
- Hot path: one `OnceLock::get` plus, when installed, one radix lookup per
  classification; no parsing or lock on the request path.
- Limit: the IP-literal `dial_url` path rejects listed addresses through
  `validate_endpoint_backend_addr`, which records no egress metric (same as
  loopback / link-local on that path).
- Not covered here: NAT64 / 6to4 embedded IPv4, CGNAT and reserved ranges are a
  separate deferred task (see below); operators can list them meanwhile.

## Verification and re-evaluation

Verified 2026-11-12 on branch `feature-10-28`:

- `cargo test -p edgion-common --lib egress` (classification precedence, policy
  independence, compile errors) and `--lib dns`.
- `cargo test -p edgion-gateway --lib` targeted tests for `classify_backend_addr`,
  `validate_backend_addr`, `validate_endpoint_backend_addr`, `first_safe_inet_addr`,
  `first_safe_resolved_addr`, egress `is_ip_allowed` / `filter_resolved_addrs`,
  DSL, GeoIP and the shipped-config parse test
  (`shipped_gateway_configs_block_vendor_metadata_endpoints`). Gateway unit tests
  share one process-wide list through `tests/unit/lib_test/test_blocked_cidr.rs`
  (`203.0.113.200/32`).
- Integration: DynamicExternalUpstream `reject_matched_blocked_cidr_rejected`
  dials the TEST-NET-3 literal `203.0.113.200`, which both integration Gateway
  configs add to the list, and requires 500/502 plus the trace-correlated
  `BackendAddrBlockedCidrNotAllowed` diagnostic. A second case uses the suite-owned
  HTTP DNS resolver to map `deu-blocked.invalid` to the same IP and requires the
  same trace-correlated denial.

Re-evaluate if a vendor changes or adds a metadata endpoint, if the classifier
gains built-in CGNAT / NAT64 / reserved handling, if the list becomes hot-reloadable
or Controller-side, or if a new egress path bypasses `classify_ip`.

## Resolution references

- `common-01` — egress classifier misses well-known cloud metadata addresses;
  promoted 2026-11-06, implemented 2026-11-12. Remaining ranges:
  [`tasks/todo/egress-classifier-cgnat-nat64-reserved-ranges.md`](../../../../tasks/todo/egress-classifier-cgnat-nat64-reserved-ranges.md).
