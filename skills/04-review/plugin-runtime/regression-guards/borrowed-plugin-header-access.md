---
name: borrowed-plugin-header-access
description: Review borrowed PluginSession request/response headers, authentication parsing ownership, and native-WAF ingestion.
knowledge_type: regression-guards
---

# Borrowed plugin header access

## Conclusion and scope

Plugins can inspect live request and response headers without first copying each
name and value into owned strings or byte vectors. The shared object-safe
boundary is `PluginSession::request_header_map()` and
`PluginSession::response_header_map()`, with typed borrowed accessors derived in
`PluginSessionHeaderExt`.

BasicAuth, LDAP, HMAC, JWT, OIDC, and JWE use the request text tier when
extracting authentication values. Native WAF request and response ingestion use
the raw iterator tier and let the transaction create only the owned state that
must survive later phases. This corrects both
`auth-borrowed-authorization-header` (the D0035 follow-up) and
`reduce-waf-header-materialization`. It does not establish allocation-free
plugin execution, credential erasure, or global wire ordering across distinct
header names.

## Rationale and evidence

The former authentication path called `header_value`, whose production adapter
constructs a `String` before parsing. The former WAF response path called
`response_headers_bytes`, allocating one owned pair vector plus separate name
and value vectors for every header; `ResponseState::build` then copied the same
bytes into transaction-owned storage.

The current source establishes this boundary:

- `edgion-gateway/src/plugins/runtime/traits/session.rs` exposes both live
  `HeaderMap` values by bare reference. A bare reference keeps the automocked
  trait object-safe; the response method returns a shared empty map before a
  response is bound.
- `edgion-gateway/src/plugins/runtime/session_adapter.rs` returns the Pingora
  request map directly and returns the bound upstream response map when present.
- `edgion-gateway/src/plugins/runtime/traits/header_ext.rs` derives borrowed
  text, per-name raw values, and request/response raw pair iterators. This keeps
  non-`'static` iterator types outside the automocked trait.
- The authentication plugins parse borrowed Authorization or
  Proxy-Authorization text while retaining their documented first-value and
  selection precedence.
- `edgion-gateway/src/plugins/http/waf/mod.rs` builds only lightweight
  `RequestHeader`/`ResponseHeader` views over the live maps. The constructor and
  `publish_response_head` consume those views synchronously before the next
  mutable session access.
- `edgion-waf/src/seclang/transaction.rs` validates the borrowed input, copies
  request state into its bounded storage, and retains response entries for
  phases 3 through 5. Removing that transaction-owned state would violate its
  lifetime and accounting contract.

This follows the repository rule that data-plane performance outranks mock
convenience. It also preserves the WAF limits and the canonical access-log owner;
the header bridge emits no tracing or header contents.

Shared outbound origin-field collection borrows Header text through `header_str`
and decoded Query text through `get_query_param_ref` until synchronous request
assembly finishes. Header selection remains first-value with `to_str` filtering;
all origin fields remain sensitive. Cookie extraction and rendered templates keep
owned results, and the final outgoing request owns its values independently.
This removes intermediate text copies, not final URL/header storage.

## Alternatives and limits

Owned accessors remain available when data must outlive the synchronous plugin
callback. ExtProc response projection now borrows the live map and copies fields
directly into its independently owned protobuf message before the transport await.
It avoids the intermediate owned pair vector and reserves the destination vector
for all field lines plus the optional status. Per-field name and byte ownership
remain necessary. The earlier WAF change left this consumer unchanged; its old
use of `response_headers_bytes()` was not a requirement of protobuf ownership.

The response map is empty before `PingoraSessionAdapter::with_response_header`
binds a live response. Consumers that must distinguish "not bound" from "bound
and empty" use `get_response_status()`. This is the same stage behavior the
owned bulk accessor exposed; it is not a new response-presence signal.

Header names remain case-insensitive. Raw iterators preserve opaque values and
duplicate insertion order for each name. `HeaderMap` does not preserve global
wire order across distinct names, so no consumer may infer it. Borrowed text
access retains `HeaderValue::to_str` rejection and first-value behavior.

The WAF still allocates one vector of small borrowed descriptors and its bounded
transaction state. Those allocations have independent purposes; this correction
removes only the redundant `1 + 2N` owned bridge allocations. Source request
headers, response headers, parsed credentials, transaction collections,
allocator residue, and transport buffers retain their own release boundaries.

## Verification and re-evaluation

Fresh focused verification on 2026-09-14 passed four `header_ext` unit tests and
the native-WAF duplicate response-header phase-3 regression. Coverage includes
trait-object dispatch, case-insensitive lookup, rejected text with raw-byte
access, and duplicate request/response values.

Independent read-only reviews of the final code, tests, benchmark, and review
knowledge returned LGTM after the benchmark's content preflight was connected to
the same descriptor constructors used by the measured paths. Workspace check,
workspace Clippy, formatting, agent documentation validation, and the SSA-force,
metrics-inventory, and Pingora-source policy guards also passed. Clippy reported
one unrelated pre-existing warning in
`edgion-tests/src/client/suites/edgion_plugins/dsl/grpc_error.rs`.

The new harness-free `waf_header_ingestion` smoke benchmark compares the former
materialized bridge with the borrowed request and response paths. Before each
measurement pair, it compares the complete ordered `(name, raw value)` descriptor
sequence and verifies that an equal-length value mutation is rejected. It also
checks the transaction outcome summary and exactly `1 + 2N` fewer allocations:

| Headers | Side | Allocations before | Allocations after | Smoke throughput before | Smoke throughput after |
|---:|---|---:|---:|---:|---:|
| 16 | request | 66 | 33 | 350,256 ops/s | 457,502 ops/s |
| 16 | response | 102 | 69 | 308,541 ops/s | 398,867 ops/s |
| 512 | request | 1,068 | 43 | 19,002 ops/s | 29,597 ops/s |
| 512 | response | 2,086 | 1,061 | 14,505 ops/s | 21,174 ops/s |

These Apple ARM64 same-process smoke values are directional evidence, not
portable thresholds. Complete descriptor equality, the negative content check,
and the allocation deltas are the stable benchmark gates; the transaction outcome
summary is supplemental. No deployed integration scenario is needed because the
wire behavior and lifecycle are unchanged.

Re-evaluate when either session map accessor, the Pingora adapter's response
binding, mockall support, HeaderMap iteration semantics, WAF transaction input
or storage, ExtProc projection ownership, or authentication extraction changes.

## Resolution references

- `auth-borrowed-authorization-header` / D0035: shared request accessor and
  authentication consumer migration.
- `reduce-waf-header-materialization`: confirmed bounded hot-path performance
  defect; request and response bridges now avoid per-header owned temporaries.
