# D0084 — Define BackendRef-Scoped Native CORS Preflight Ownership

## Meta

- Status: needs-discussion
- Severity: medium
- Discovered in: B0124 Gateway API CORS filter analysis
- Category: correctness-routing
- Canonical task: [`tasks/todo/define-backendref-native-cors-preflight-ownership.md`](../../../todo/define-backendref-native-cors-preflight-ownership.md)

## Evidence

The request-filter path recognizes native CORS only on the selected route rule. A preflight with CORS only on a `backendRef` therefore reaches the global headerless local-response path before backend selection. The backend CORS runtime is nevertheless compiled and runs for actual responses after selection, leaving backend-scoped CORS partially active and its preflight behavior unreachable. Mixed weighted backends may also carry different CORS policies.

## Why deferred

A correction must choose and enforce one policy: reject backend-scoped native CORS, or define deterministic backend selection and CORS ownership before preflight interception. That decision crosses routing-stage ownership, backend pinning, Controller validation/status, and Gateway API implementation-specific semantics; it does not belong in B0124's representation-only edit.

## Integration-test impact

The canonical technical task owns backend-only and mixed-backend cross-process coverage together with the implementation. No separate integration-only issue is needed unless the future design cannot add or register standalone coverage without modifying existing frozen tests.
