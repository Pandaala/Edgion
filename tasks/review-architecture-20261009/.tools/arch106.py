import record
body = '''
# Enforce policy ancestor capacity after protected history and owned entries are composed

## Problem and current ownership

BackendTLSPolicy and EdgionBackendTrafficPolicy limit locally discovered Gateway ancestors to 16, but neither handler nor the canonical native-status projection budgets the final list containing protected observations from other controllers. A valid observed list of 16 foreign ancestors followed by one newly discovered, provably owned Gateway produces 17 persisted ancestors. This exceeds both current CRD schemas. The BackendTLSPolicy contract additionally requires a full list to make the affected policy attachment unimplementable and to report that on related resources; suppressing a status write alone would not satisfy that requirement.

The cold dependency resolver owns discovery, the policy handlers own local conditions and resolved target preparation, and status_ownership::native_status_projection owns the protected observed/local merge. KubernetesResourceController owns the resource-version-fenced status write. The list budget currently sits only at discovery, before the protected merge it must constrain.

## Evidence and concrete trigger

- edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/mod.rs:191-231 caps the locally discovered Gateway map at MAX_POLICY_ANCESTORS = 16. It does not inspect observed ancestors.
- handlers/backend_tls_policy.rs:493-735 retains every foreign ancestor at 602-609 and appends each locally computed ancestor at 727-732. handlers/edgion_backend_traffic_policy.rs:540-704 independently retains foreign entries at 586-593 and appends local entries at 690-701.
- status_ownership.rs:176-261 filters computed entries through the current ownership proof, preserves protected observed entries at 231-254, and unconditionally extends the result with owned entries at 255. Thus foreign preservation is correct, but the final ancestor capacity is unchecked.
- processor.rs:738-790 uses this projection as the persisted status. KubernetesResourceController's worker and patch helper at edgion-controller/src/conf_mgr/conf_center/kubernetes/resource_controller.rs:707-760 and 812-868 submit the computed status with the observed resourceVersion; they do not reduce the list.
- config/crd/gateway-api/gateway-api-standard-v1.6.2.yaml:432-472 states the 16-ancestor maximum, forbids additions to a full list, and requires unimplementable-policy reporting on related resources; its schema sets maxItems: 16 at 709. config/crd/edgion-crd/edgion_backend_traffic_policy_crd.yaml:354-362 independently sets maxItems: 16 at 357.

The trigger requires one otherwise valid policy targeting a Service, an existing Service->Route->Gateway dependency, matching published/raw GatewayClass and Gateway incarnation proofs, and 16 distinct valid foreign ancestor observations. The local discovery map has one member and passes its cap. The handler and canonical projection then produce a 17-member persisted list. Schema rejection of that status PATCH is a static prediction from the submitted shape and current schema, not a reproduced Kubernetes incident.

Status-write failure does not withdraw the prepared policy. Gateway BackendTLSPolicyStore::is_valid_candidate at edgion-gateway/src/backends/policy/backend_tls/backend_tls_policy_store.rs:222-272 checks target/class and TLS preparation without inspecting ancestor capacity. EdgionBackendTrafficPolicyStore::prepare_policy at edgion-gateway/src/backends/policy/backend_traffic/store.rs:224-248 likewise derives eligibility from validation, resolved target class and canonical targets. Its ineligible candidates are excluded from the reverse index at 281-334; eligible=false is not an explicit denied-occupancy result. BackendTLSPolicy's existing invalid buckets at 165-183 have different semantics. Neither store can be treated as a shared fail-closed shortcut.

## Bounded improvement and acceptance

Make the full-list capacity outcome part of cold policy preparation, at the boundary that combines protected observed history with canonical replacement of this controller's owned entries. Count retained foreign entries and retained own entries whose ownership cannot currently be established. Updating an existing owned composite key at capacity must remain possible; adding a new key must require an available slot. Preserve every protected entry and its identity instead of reclaiming capacity from another writer.

Carry the same outcome into local policy/attachment eligibility and related-resource diagnostics through the existing serial dependency workqueues. For BackendTLSPolicy, retain canonical target occupancy and use the existing invalid-policy preparation boundary where appropriate. For EdgionBackendTrafficPolicy, explicitly represent or gate the unimplementable attachment in the existing related-resource preparation boundary: merely clearing resolved_target_class or excluding a candidate can expose ordinary policy fallback and is insufficient. The exact policy-wide versus per-ancestor representation requires bounded design review; it must prevent a new capacity-refused Gateway attachment from being treated as successfully implemented while retaining legal updates to represented owners. Do not infer a generic all-resource serving gate from status persistence.

Acceptance requires no emitted policy ancestor list above 16, unchanged foreign preservation and class/incarnation proofs, capacity refusal reported on the affected related resources, and no silently active attachment whose required new ancestor cannot be represented. Preserve apply replacement, dropped-reference requeue, target dependencies/conflict ownership, one queue/one serial worker, inline ordered status IO and resourceVersion fencing. A freed slot must recover through the existing reprocessing path. Keep FileSystem/etcd single-controller behavior and the private controllers[] envelope's separate policy distinct.

Add focused tests for 16 foreign entries plus one owned Gateway, 15 foreign plus one owned, an existing owned-key update at 16, protected unknown-ownership history, removal/recovery, and both policy families. Test both persisted projection and actual policy/related-resource preparation, since a list-only test cannot prove refusal. Preserve the existing foreign-entry, condition-transition-time and deployment-identity tests.

## Alternatives, prior decisions and limits

A final PATCH length guard is a useful defensive check, but cannot be the complete change: it prevents schema rejection while leaving current local eligibility untouched. Truncating or evicting foreign entries violates the explicit foreign-status-entry-preservation contract. The existing local discovery cap cannot account for observations owned by other controllers. Copying the private controllers[] envelope's accepted suppress-write/continue-local behavior would conflict with the policy ancestor contract.

The original foreign-status-entry-preservation page correctly requires preservation and terminal full-list refusal. Its statement that the native path complies covers foreign retention; it does not establish the missing final-capacity behavior. It cites vendored Gateway API v1.5.0 paths that are absent from this baseline; the current v1.6.2 schema carries the same normative full-list text. Canonical-parent-status-ownership and Kubernetes status-snapshot concurrency remain applicable to writer/class proof and optimistic-concurrency fencing; neither grants capacity reclamation or makes a failed PATCH a serving gate.

This finding is confined to the two policy ancestor lists. It makes no claim about Route/EdgionTls parents, the private 32-entry envelope, unrelated more-than-16 local-discovery behavior, memory/performance measurements, a production incident, or executed conformance/API tests. Existing selected tests cover foreign-plus-owned merging and ownership/condition preservation, but do not cover a full foreign ancestor list. This review is static and read-only.
'''
print(record.candidate('ARCH-106','policy-ancestor-final-capacity','PART-405','/root/analyze_conversion','behavior-defect','P2',body))
