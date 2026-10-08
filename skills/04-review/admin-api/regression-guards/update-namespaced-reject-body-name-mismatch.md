---
name: update-namespaced-reject-body-name-mismatch
description: Review Admin URL/body identity admission, namespace omission, and immutable identity on updates.
knowledge_type: regression-guards
---

# Admin writes preserve URL and stored resource identity

## Contract and implementation

Namespaced Admin create/update reject an explicit body namespace that differs
from the URL. Both namespaced and cluster updates reject an explicit body name
that differs from the URL; collection creates still require a body name.

Omitted body name on update defers to the URL-addressed current resource. The
shared `merge_operator_update_values` writer merge preserves current metadata,
including name, namespace and UID; FS/etcd additionally retain the newly assigned
resourceVersion through `merge_operator_update_yaml`. Thus absence of a name in
the parser output alone does not demonstrate an identity defect.

Namespaced create/update use `admit_namespace` before per-kind parsing and
persistence: absent/null namespace becomes the URL namespace, matching explicit
namespace is retained, and empty/nonstring/conflicting namespace or absent/non-object
metadata is rejected. Only namespace changes in the complete raw document;
source API version and unknown source fields remain available to downstream
validation and Kubernetes HTTPRoute raw forwarding. Original raw validation and
update precondition capture run before this admission step.

Before this correction, a namespace-less create succeeded but wrote namespace-less
content to a namespaced storage location. Strict FS/etcd ingest then rejected its
identity. Binding namespace at Admin admission fixes newly created records and
their subsequent update lifecycle. It does not repair already malformed stored
records: update merges intentionally preserve current metadata identity.

This is an admission boundary, not permission to normalize stored objects.
Direct FS/etcd ingest continues to reject key/content mismatches; see
[etcd identity guard](../../controller/regression-guards/etcd-key-content-identity.md).
No cluster-resource namespace or reference default is introduced.

## Evidence and limits

- `edgion-controller/src/api/namespaced_handlers.rs`: create/update admission.
- `edgion-controller/src/api/cluster_handlers.rs`: cluster name guard, unchanged.
- `edgion-controller/src/conf_mgr/conf_center/common.rs`: immutable identity merge.
- Existing `update_identity_tests` retain explicit identity and stale-version cases.
- `namespace_admission_tests` covers JSON/YAML create/read/update persistence,
  omitted/null/matching namespaces, mismatch rejection without writes, preserved
  UID/name/preconditions, raw-field preservation, and FS init/runtime worker publication.
- Unit coverage does not exercise a live Kubernetes API server or etcd service.

Re-evaluate when admission, raw forwarding, writer merging, or storage identity
rules change. Historical matching/mismatch tests did not establish omitted-create
namespace binding; the earlier blanket fixed verdict had that coverage limit.

## Resolution references

- `controller-02` — 2026-05-24: explicit update identity mismatch rejection.
- `v1-01` — 2026-05-29.
- Resource-reference contract R4c: Admin namespace admission.
