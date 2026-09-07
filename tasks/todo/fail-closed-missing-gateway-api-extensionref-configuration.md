# Fail Closed on Missing Gateway API ExtensionRef Configuration

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Reject a Gateway API filter whose `type` is `ExtensionRef` but whose payload is absent on every ingest path, with a Gateway fail-closed backstop if malformed data still arrives.

## Requirements

- Define one canonical shared validation decision for the filter type/payload pairing and supported group/kind contract across HTTP and GRPC routes.
- Apply it to rule and backendRef filter lists, including delegated or effective rules.
- Match Kubernetes CEL behavior for filesystem and etcd input: a missing payload is invalid rather than a silently absent filter.
- Make Controller status mark the owning rule index invalid and surface `PartiallyInvalid` while valid sibling rules remain active.
- Make Gateway construction independently install the canonical generic 500 request gate if malformed data reaches it; never omit the filter silently.
- Preserve valid EdgionPlugins resolution, dangling-target request-pinned 500 behavior, unsupported-kind ExtensionRef attribution/logging, backend-WAF restrictions, immutable publication, redaction, canonical access-log ownership, no request-path tracing, and response streaming.
- Do not modify existing integration-test files.

## Acceptance criteria

- Focused shared-resource tests cover present/missing payload and supported/unsupported group-kind combinations.
- HTTP and GRPC Controller tests cover missing payload at rule and backendRef scope, exact invalid-rule selection, `PartiallyInvalid`, and valid siblings.
- Gateway builder/runtime tests prove malformed input returns the generic 500 before upstream and retains bounded ExtensionRef-attributed diagnostics.
- A standalone non-Kubernetes integration scenario is added only if unit tests cannot faithfully prove the ingest boundary.
