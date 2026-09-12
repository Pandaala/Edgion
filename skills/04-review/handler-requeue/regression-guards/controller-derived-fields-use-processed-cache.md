---
name: controller-derived-fields-use-processed-cache
description: Review cross-kind Controller lookups of processed fields, including the policy-independent AI admission source view.
status: fixed
finding_id: ctrl-04
closed: 2026-08-09
knowledge_type: regression-guards
---

# Controller-derived fields use the processed cache

## Conclusion and scope

A cross-kind decision that consumes fields populated by `preparse` or `parse`
reads the processed `ServerCache` through `ProcessorObj::get_cached_any`.
`RawStore` remains the source for unfiltered input and `on_change` self-lookups,
where the new processed generation has not yet been saved. Choose the lifecycle
view required by the decision; do not combine existence from one with content
from the other.

The Controller no longer owns plugin execution policy. HTTPRoute's
`ai_proxy_admission_state` reads processed EdgionPlugins for enabled AiProxy
source configuration, with no Block/Bypass state. Policy-sensitive route
admission belongs to the Gateway's captured class scope. See
[scoped policy publication](../../plugin-runtime/regression-guards/scoped-policy-runtime-publication.md).

## Rationale and evidence

RawStore contains Kubernetes, filesystem or etcd input. ServerCache contains the
handler-filtered object, including resolved references, credentials and status,
that conf_sync publishes. Typed projection avoids serializing resolved secret
material into diagnostics or responses.

Controller HTTPRoute processing transports `resolved_ai_admission` facts for
original and delegated rules, and uses the shared pure AI validator for intrinsic
source diagnostics. Those diagnostics do not drop a rule that a current Gateway
policy might Block. Independent unsupported backend WAF/native CORS and delegation
rejections retain their existing rule/attachment boundaries.

Source: `processor.rs::{get_raw_any,get_cached_any}` and
`handlers/http_route.rs::{ai_proxy_admission_state,resolve_ai_admission}` under
`edgion-controller/src/conf_mgr/sync_runtime/resource_processor/`.
The lifecycle authority is
[resource processing](../../../01-architecture/01-controller/05-resource-processor.md).

## Historical boundary

`ctrl-04` was closed on 2026-08-09 after AI admission read raw plugin input rather
than Controller-stamped policy state. That implementation and its stamp-specific
tests were removed by the authorized D0095 migration on 2026-09-11. Inbound stamp
clearing, global Block scans and Bypass counting are obsolete guidance. Their
removal does not invalidate the general processed-cache rule or retrospectively
change the original resolution.

## Verification and re-evaluation

During the migration, the focused Controller HTTPRoute module passed 38 tests and
the Controller all-target check passed. These verify the extraction checkpoint;
the final D0095 disposition and current focused regression evidence are recorded
in [scoped policy publication](../../plugin-runtime/regression-guards/scoped-policy-runtime-publication.md).

Re-evaluate when a field changes lifecycle ownership, processed publication APIs
change, or a caller needs unfiltered source input instead of the conf_sync view.

## Resolution references

- `ctrl-04` — processed-cache correction, closed 2026-08-09.
- D0095 — Controller stamp authority removed; scoped publication correction verified and closed.
