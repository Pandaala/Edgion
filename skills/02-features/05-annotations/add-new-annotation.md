---
name: add-new-annotation
description: Developer workflow for adding or changing an edgion.io metadata annotation, options key, or related label.
---

# Add or Change an `edgion.io/*` Key

Read the overview and the relevant reference first. Not every `edgion.io/*` key is a
`metadata.annotations` entry: some live in listener/policy `options`, some are labels,
and some are system/test-only.

## 1. Define the Contract

Record these decisions before coding:

| Decision | Examples |
|----------|----------|
| Carrier | `metadata.annotations`, `tls.options`, policy `spec.options`, label |
| Resource/scope | Gateway, HTTPRoute rule set, Service, EdgionTls, synthetic EndpointSlice |
| Owner | Operator-facing, Controller-written, Gateway-written, test-only |
| Parse owner | Shared resource preparse, Controller resolution, Gateway runtime |
| Value grammar | Exact enum, tolerant boolean, integer/range, ordered reference list |
| Invalid behavior | Reject/status failure, WARN/INFO + inherit/default, or ignore |
| Hot reload | Whether an update affects existing listeners/sessions or only new ones |

Do not impose one universal parser policy. Existing contracts deliberately differ:
`enable-http2` accepts several falsy tokens, `cert-provider` requires exact
case-sensitive `EdgionTls`, and malformed reference-bearing DNS options may produce
`ResolvedRefs=False`. Preserve the behavior appropriate to the risk.

## 2. Locate Ownership Before Adding a Constant

For product metadata annotations, prefer a named constant in
`edgion-resources/src/constants/annotations.rs::edgion`. That file has no `test_only`
submodule. Options and labels may currently have constants beside their owning schema or
consumer; follow the nearest established pattern and avoid creating duplicate literals.

Search both definitions and consumers:

```bash
rg -n 'edgion\.io/|YOUR_KEY' edgion-resources edgion-controller edgion-gateway edgion-tests config skills
```

If an existing key is still an inline literal, centralizing it can be part of the same
minimal change when all consumers are updated and tests cover the exact string.

## 3. Choose the Parse Path

### Shared resource preparse

Use this when the normalized result should travel with the resource through conf_sync.
For example, HTTP route annotation parsing lives in
`edgion-resources/src/resources/http_route_preparse.rs`; it is not automatically a
Controller-handler responsibility. Add only the parsed runtime field needed by the
Gateway and follow the schema/transport rules in
`../../01-architecture/05-resources/00-schema-transport-contract.md`.

### Controller resolution

Use the relevant handler/service when the result depends on other resources,
ReferenceGrant, Secrets, or status conditions. Current consumers include:

- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/gateway.rs`
- route handlers under the same directory
- `edgion-controller/src/services/dns_resolver/`

Keep controller-resolved fields serialized for conf_sync and excluded from the CRD with
`#[schemars(skip)]`; do not use `#[serde(skip)]` when the Gateway needs the value.

### Gateway runtime

Read directly when the resource annotation itself is the runtime contract. Locate the
actual current consumer rather than assuming one listener file:

- Gateway HTTP/2/redirect: `edgion-gateway/src/runtime/server/base.rs`
- Gateway-derived settings: `edgion-gateway/src/runtime/gateway_info.rs`
- TLS provider selection: `edgion-gateway/src/runtime/matching/tls.rs`
- TCP/TLS/UDP route consumers: `edgion-gateway/src/routes/{tcp,tls,udp}/`
- Log projection: `edgion-resources/src/resources/common/log_labels.rs`

## 4. Validation Boundaries

- For an Edgion-owned structural CRD field, update its schema and run the CRD coverage
  guard. An annotation remains a map entry; adding a Rust constant does not add a typed
  CRD field.
- Do not patch vendored Gateway API/native Kubernetes CRDs merely to validate one Edgion
  annotation. Use existing admission/status/runtime parsing unless the project explicitly
  introduces a validating webhook contract.
- Reference-bearing keys must use the project's ReferenceGrant and status conventions.
- System-written annotations need a clearly documented writer and must not be presented
  as operator configuration.

## 5. Documentation and Tests

Update all applicable files:

- `00-annotations-overview.md` for quick lookup
- `references/annotations-metadata.md` for metadata annotations
- `references/annotations-options-and-labels.md` for options/labels
- `references/annotations-system-and-test-keys.md` for reserved/test keys

Tests should cover absent, valid, invalid, boundary, precedence/inheritance, and hot
update behavior as applicable. Add a targeted integration scenario when the key changes
observable routing, TLS, DNS, plugin, or logging behavior. Use
`../../05-testing/01-integration-testing.md` for the harness.

## Review Checklist

- The carrier and exact case-sensitive key/value grammar match the consumer.
- Scope and precedence are documented, including explicit false vs absent when tri-state.
- Invalid input behavior is intentional and tested; no blanket fallback rule was copied.
- Controller-resolved transport fields follow schemars/serde/redaction invariants.
- Current and legacy keys are not both carried forward without an explicit requirement.
- Quick lookup, detailed reference, source constant/consumer, and examples agree.
