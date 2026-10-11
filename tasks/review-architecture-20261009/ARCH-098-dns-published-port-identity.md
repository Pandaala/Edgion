---
id: ARCH-098
status: candidate
kind: behavior-defect
priority: P2
partition: PART-435
analyst: /root
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# ARCH-098: Bind DNS slice publication identity to the port fields the Gateway consumes

## Current ownership and supported change

`edgion-controller/src/services/dns_resolver/service.rs::slice_fingerprint` represents sorted resolved IPs and sorted Service port numbers only (`481-494`). Its comment deliberately excludes port name, protocol and appProtocol because an older routing assumption treated the number as sufficient. `slice_builder::endpoint_ports` nevertheless publishes all four fields. `can_skip_owned_slices` checks fingerprint equality, required address families, canonical slice names, Service labels and owner UID, but not these port contents (`420-462`).

For one unchanged Service UID, first publish a controller-mode ExternalName Service's port80 named old under TCP. Rename that port to new while keeping port80 and the same resolved IPs. The next successful DNS tick has the same fingerprint. Its observed slice has the right family/name/owner, so the producer skips SSA and leaves its EndpointPort name as old. Every unchanged subsequent tick can repeat this decision.

The current Gateway discovery coordinator compiles Service sections against EndpointSlice port names and protocols. `matching_endpoint_ports` and `endpoint_port_matches` require an exact named-port match and protocol equality (`1589-1614`). The renamed Service therefore has no slots from its stale generated slice. TCP/UDP/TLS selection has no Gateway DNS fallback and can remain unavailable; native HTTP/gRPC controller mode may bridge to DNS when no ready slots remain, so this report does not claim a universal HTTP outage. A protocol-only change has the same omitted-input problem. AppProtocol alone is stale output metadata, not the demonstrated name/protocol routing witness: Gateway's section appProtocol is derived from the current Service.

## Bounded improvement

Make the existing successful-publication identity include the canonical EndpointPort projection actually produced by the slice builder: name, port number, protocol and appProtocol, with deterministic ordering and unambiguous optional-field encoding. Share the existing pure mapping or its projection within this DNS owner so adding a meaningful published port field cannot silently leave skip identity behind. Keep sorted/deduplicated IP identity and existing Service-UID/family checks. Do not include resolved-at timestamps or unrelated Service metadata that would force unchanged periodic writes.

An alternative is comparing every observed desired slice's published port projection before allowing a skip. It can additionally repair out-of-band edits, but is a broader observed-content contract and should be assessed separately. The scoped problem can be closed by fixing the missing source port inputs without redefining all output reconciliation.

Preserve forced SSA, canonical slice names and Service owner references, complete desired-family success before updating the remembered fingerprint, retries after partial apply failure, last-good slices on DNS failure, no-port/no-UID behavior, grant-gated tenant resolver selection, administrative default authority, failed prepared-generation fallback and reuse, namespace scoping, service pruning and leader-job lifecycle. No handler IO, new workqueue, background cache, metric or request-path allocation is needed.

## Authority, existing tasks and distinguishing evidence

Current backend architecture and the original named-port section review require exact serving-port proofs, with L4 consuming controller slices and native HTTP/gRPC retaining their specified DNS bridge. The old number-only source comment's rationale conflicts with those actual consumers; it is evidence of the mistaken ownership assumption, not authority to relax current Gateway port matching. Preserve the deliberate Service port-number mapping rather than switching ExternalName output to targetPort.

The existing reference-contract task has already repaired observed Service UID and required-family skip checks. Its explicitly open obsolete-family deletion and async output-authority/GC residuals are distinct and remain linked in the partition report. This finding adds no new claim for those known residuals. Searches of current review artifacts, original knowledge and that task found no matching port-name/protocol fingerprint correction.

All three DNS implementations and all915 service-test lines plus161 slice-builder-test lines were read. Actual in-process client tests verify ownership/family repair, force=true, failed preparation reuse, tenant/default permission and partial dual-family retry, but not an unchanged-IP/number name or protocol edit. Actual Gateway section fixtures verify name matching and heterogeneous siblings; current source establishes protocol matching.

Future focused verification should publish a slice, mutate only a Service port name or protocol, retain the same Service UID/IPs/numbers and verify a repair PATCH with the new port projection, followed by no PATCH on the next identical tick. Verify the repaired slice produces slots for the changed Gateway section and preserve ordinary HTTP DNS fallback as a control. Retain unchanged-order stability, multiport/mixed-family, UID repair and partial-failure tests. Use existing in-process Kubernetes client fixtures; no new mock framework or network topology is required.

Classification: behavior-defect, P2. Static source and test-body review only; no executed reproduction, test, build, benchmark or incident claim. No prompt cancellation/revocation, complete observed-content repair or byte-bound guarantee is added.
