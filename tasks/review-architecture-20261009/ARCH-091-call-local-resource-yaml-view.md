---
id: ARCH-091
status: confirmed
kind: architecture-debt
priority: P3
partition: PART-394
analyst: /root/analyze_conversion
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# ARCH-091: Reuse the call-local raw YAML view at resource-ingress preparation

## Problem and current ownership

`edgion-controller/src/conf_mgr/conf_center/common.rs::parse_resource_content_inner` repeatedly parses the same complete input into `serde_yaml::Value` to answer source-identity questions and prepare an advisory diagnostic. Those independent passes own no distinct authoritative generation. They repeat document traversal and allocation within one cold resource-ingress call while making source-version decisions depend on several separately reconstructed views.

At `common.rs:225`, the public `validate_resource_api_version` wrapper parses a complete Value for registered Gateway API kinds (`316-328`). The private caller then parses another Value for beta detection (`227-230`). For GRPCRoute, TLSRoute, BackendTLSPolicy, TCPRoute and UDPRoute, the alpha condition parses another Value (`252-263`); selected alternate input is then parsed again into the mutable source document (`265`). Thus valid canonical Gateway/HTTPRoute/GatewayClass/ReferenceGrant input takes two full Value parses plus typed decoding; canonical input for the five route/policy kinds takes three plus typed decoding; selected offline alpha input takes four full Value parses before schema validation and `from_value` conversion. These are static call-count facts, not measured latency or peak-memory results.

For EdgionPlugins, EdgionStreamPlugins, EdgionConfigData and LinkSys, the unconditional beta-detection parse precedes typed decoding, and `populate_unknown_fields_non_fatal` reparses the complete text into a Value solely for the advisory JSON view (`350-366`). That gives two raw Value parses plus typed decoding on their supported FileSystem/etcd ingress path. The issue is repeated preparation inside this function, not placement of strict validation in the Controller.

## Actual consumers and bounded value

FileSystem and etcd resource controllers call `parse_resource_content_offline` for initial documents and runtime applies before publishing raw state. The shared direct parser is also used by Admin source adaptation and Kubernetes CRUD conversion of selected beta sources. The raw TypeMeta guard is necessary because typed serde can discard submitted source identity, and static alpha input must be checked against its declared source schema before canonical conversion.

A change to a supported source version or diagnostic currently has several complete-document preparation sites to keep coherent. Reusing one call-local view reduces that repetition and gives source identity one local input owner. Bulk startup/relist performs these calls for each resource, but no benchmark, throughput gain, global RSS estimate or request-path improvement is claimed.

## Bounded improvement

Parse the raw YAML Value once where the current invocation requires it and borrow that same view for source identity, beta/alpha selection and diagnostic projection. Use the existing `validate_gateway_api_source(kind, &value)` logic rather than calling a text-parsing wrapper from the already-prepared private path. Move the same owned Value into the existing selected-alpha conversion after source-schema validation. Let the unknown-field helper consume a borrowed raw view and retain its current non-fatal YAML-to-JSON conversion policy.

Keep `validate_resource_api_version` independently strict for its public standalone callers. Do not share mutable memo state across Admin/backend calls or trust an earlier ingress boundary on behalf of another producer. Keep ordinary typed and beta-source serde decoding from the original text; replacing all typed decoding with `from_value` is outside this proposal because text and Value deserializers are distinct paths with duplicate-field, tag and error-location contracts to preserve.

Retain current failure precedence: raw syntax and source-GVK rejection precede typed decoding; alternate-source schema checks precede canonical conversion; advisory conversion failure leaves the already-parsed object's findings unchanged. Drop the raw view before text typed decoding when no conversion/diagnostic consumer remains, and release it at the last necessary consumer otherwise. It must never become a field on a typed resource, runtime generation, cache or transport object. This is private call-local preparation, not a new generic ingress framework or serialized cache.

## Alternatives and preserved invariants

Leaving the passes independent preserves repeated document construction. Eliminating source validation, converting through JSON first, or replacing typed source adapters with a TypeMeta rewrite would reduce work by relaxing or changing an accepted contract and is not an acceptable alternative. A reusable parsing service or shared DOM cache adds lifetime/invalidation ownership that this scoped change does not need.

Preserve the finite code-owned source inventory, exact raw kind/group/version validation, canonical typed models, existing beta adapters, selected alpha OpenAPI/reference validation, unknown-field path ordering and mode-specific diagnostic policy. Keep Controller/Gateway validation parity, typed unsupported plugin sentinels, credential/redaction rules, serial workqueue processing, authoritative raw stores and inline status IO unchanged. No raw document contents are added to logs or metrics.

## Original decisions, tests, and limits

The API-version-upgrade contract requires one canonical model and separately verified source adapters; existing TLSRoute/BackendTLSPolicy ingress guards carry an explicit current-source update superseding their historical v1-only language. Their historical rejection results do not authorize dropping current adapters. The runtime-ignored-field decision requires advisory detection on raw input before typed round trips discard unknown keys; it supports retaining that diagnostic boundary rather than deleting it.

The complete shared parser tests cover source-GVK rejection, canonical/beta conversion, metadata/status preservation, offline alpha bounds, clean and typo paths for all four diagnostic kinds, open Misc payloads, and non-fatal JSON conversion failure. The last case directly exercises the helper with an already-parsed object; its comments explicitly qualify that exact malformed mapping-key shape as rejected earlier by the current typed ingress. Preserve that distinction when adapting the helper test.

Future validation should retain those behavior tests and check original-text duplicate/tag/malformed-input and failure-order controls for affected branches. It should establish equivalent accepted resources, canonical metadata/status and diagnostics without freezing a private helper shape solely to count parsing calls. The pinned serde_yaml APIs and current source were inspected; no test, build, application, network operation or benchmark ran. This finding does not claim that current parsing accepts invalid source versions or that cold work is happening on the Gateway request path.
