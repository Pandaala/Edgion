---
name: kubernetes-preflight-node-collection
description: Use when reviewing Kubernetes preflight collection parsing and kubectl diagnostics.
knowledge_type: regression-guards
---

# Kubernetes preflight accepts kubectl Node collections

## Conclusion and scope

`preflight_kubernetes_cluster` in `edgion-tests/k8s/scripts/run_k8s_integration.sh`
accepts both a typed `v1/NodeList` and a generic `v1/List` containing `v1/Node`
objects. The collection must have an array of objects. Generic lists containing
other resource kinds are rejected. Existing nonempty, Ready-node, and architecture
checks remain in place.

Only stdout from `kubectl get nodes -o json` enters the JSON parser. Warnings and
errors on stderr remain visible in the run log; a successful warning is not an
invalid JSON document. A nonzero kubectl exit still fails preflight.

## Rationale and evidence

kubectl may produce a generic `List` rather than the API's typed `NodeList`.
The previous kind-only guard rejected a real Ready OrbStack node collection.
Separately, merging stderr into stdout corrupted otherwise valid JSON when
kubectl emitted a warning. Neither behavior establishes a cluster defect.

## Alternatives and limits

Do not accept arbitrary objects merely because they contain `items`, and do not
discard stderr to make parsing succeed. Typed NodeList items may omit their
individual type metadata; generic List items must identify themselves as Nodes.
Passing preflight establishes cluster prerequisites, not integration-suite success.

## Verification and re-evaluation

`bash edgion-tests/k8s/scripts/common/tests/kubernetes_preflight_test.sh` passed
on 2026-09-08. It covers both collection forms with and without stderr warnings,
retained diagnostics, malformed JSON/items, a non-Node list, failed kubectl,
missing/mismatched context, unreachable API, empty nodes, and Ready-node checks.
The extracted preflight function also passed against the real `orbstack` context
on the same date: one Ready node out of one, architecture `arm64`.
Re-evaluate if the runner changes command output formats or stream handling.
The earlier aborted PolicyIntegration run remains incomplete; these parser tests
do not supply its missing dynamic end-to-end evidence.

## Resolution references

- `CAND-B-1` — corrected the NodeList-only assumption.
- `CAND-B-2` — separated JSON stdout from successful stderr warnings.
