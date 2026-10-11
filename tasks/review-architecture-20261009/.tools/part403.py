import record

bodies = {
'ARCH-099': '''
# ARCH-099: Project only a target's policy timestamps under the existing snapshot lock

## Current ownership and concrete cost

BackendTLSPolicyState owns a bidirectional target-reference manager and a separately locked timestamp map. In backend_tls_policy_ref.rs:94-122, is_winner first obtains the target's references and preserves a fast return for zero or one reference. For a conflicting target, it clones the entire timestamp HashMap, including owned policy-key strings, before projecting timestamps into the target-local entries vector.

Both BackendTLSPolicyHandler::update_status and EdgionBackendTrafficPolicyHandler::update_status use this implementation for real policy arbitration. Their runtime instances remain separate. Eligible policy registration, removal and sibling requeues maintain each instance's inputs.

Consider many independent conflicting policy pairs, each targeting a different Service. A decision for one pair needs two timestamps but copies the keys and timestamps of every registered policy in that instance. A reconciliation pass over those pairs repeats the inventory-wide copy. This couples target-local arbitration work to unrelated policy population; it is more than a small target-local Vec or sort. The source proves the copies, not a measured latency, throughput or heap regression.

## Bounded improvement

Keep the existing reference lookup and zero/one-reference shortcut. Acquire one timestamp read guard and build the same target-local entries vector from only the referenced policy keys and copied DateTime values. Release the guard before sorting. Keep the comparator and winner selection unchanged.

This retains one coherent view of timestamp values while eliminating the owned snapshot of unrelated entries. It requires no additional index, cached winner, public abstraction, new lock, or alternate arbitration implementation.

The target-reference lookup and timestamp lock are still separate operations. This proposal does not claim or create an atomic snapshot spanning both owners.

## Required semantics and risks

Preserve oldest creationTimestamp first, namespace/name key tie-breaking, Some timestamps before missing timestamps, the existing zero/one-reference behavior, eligible-target filtering, typed target and section isolation, and existing sibling requeues. set_timestamp(None) must continue to remove a previous timestamp rather than retain stale state.

The read guard must cover every relevant timestamp lookup. Replacing the full clone with separately locked get_timestamp calls would restore the inconsistency that the historical correction addressed. Do not hold the guard through sorting or handler work.

## Original knowledge and deduplication

backend-tls-winner-single-snapshot.md records the previous timestamps.read().clone() correction. Its consistency requirement remains applicable; retaining unrelated owned entries is an implementation detail that can be replaced by one guarded projection.

The BackendTLSPolicy architecture specifies oldest-wins and tie-breaking. ARCH-055 concerns timestamp precision across source/status/transport boundaries and remains independent. This proposal changes neither precision nor ordering.

## Future verification

Retain the existing target-key, single-policy, timestamp ordering, equal-timestamp tie, section isolation and sibling tests. Add focused cases for timestamp removal and unrelated timestamp entries. Source inspection must show one guarded projection and no full timestamp-map clone. No benchmark or production performance result is claimed by this review.
''',
'ARCH-100': '''
# ARCH-100: Reject invalid LabelSelector expression cardinality before namespace matching

## Current ownership and supported witness

NamespaceStore supplies committed Namespace labels to the existing matches_selector evaluator. In namespace_store.rs:116-140, evaluate_expression converts absent values into an empty vector and evaluates NotIn using labels.get(key).is_none_or(...). An absent or empty values array therefore makes NotIn match every label set.

A Gateway listener can select namespaces using:

{"matchExpressions":[{"key":"team","operator":"NotIn"}]}

The same problem occurs with "values": []. With a committed route Namespace containing team=restricted, the evaluator returns true. For an otherwise eligible cross-namespace parent reference, route_utils::listener_allows_route_namespace accepts that Namespace and validate_parent_ref_accepted uses the result in listener acceptance.

This is a supported typed path: RouteNamespaces.selector is retained as serde_json::Value, the Kubernetes Namespace watcher commits actual Namespace labels, and GatewayHandler::validate does not validate expression cardinality. The vendored standard Gateway API v1.6.2 Gateway v1 and v1beta1 schemas require key and operator but do not impose a values minimum or operator-dependent CEL check. Their descriptions explicitly require nonempty values for In/NotIn and empty values for Exists/DoesNotExist. No live API-server or traffic execution is claimed.

## Bounded improvement

Keep the existing namespace store and canonical local evaluator. Validate expression values at the evaluate_expression boundary before matching:

- In and NotIn require a nonempty array of strings.
- Exists and DoesNotExist permit absent values or an empty array, and reject nonempty values.
- Invalid values shapes return false rather than being reduced to a valid-looking empty list.

Continue matching valid expressions with borrowed input. Do not introduce a selector framework, another Namespace cache, new I/O, metrics, or logs.

The pinned kube-core 2.0.1 Selector conversion is not a sufficient replacement: its TryFrom<LabelSelectorRequirement> implementation rejects absent values for In/NotIn but accepts Some(empty), and it ignores values for Exists/DoesNotExist. Native conversion alone would leave the decisive cardinality defect.

## Required semantics and scope

Preserve valid NotIn behavior when the label key is absent, valid In/Exists/DoesNotExist behavior, AND composition, valid empty-object selectors, existing unknown-operator rejection, and existing Namespace-label refresh and missing-label fallback behavior.

This candidate does not redefine the evaluator's separate top-level null behavior or route_utils' separate missing-selector branch. It does not change All/Same handling or introduce a new routing owner. It concerns expression cardinality in the current evaluator.

## Original knowledge and deduplication

The Gateway architecture, feature reference and vendored Gateway API schema establish namespace-selection semantics. Current source and tests contain valid expression coverage but no rejection coverage for these invalid cardinalities. No existing ARCH candidate found in the review artifacts owns this expression-validation cause.

## Future verification

Add evaluator regressions for absent and empty NotIn values with both present and absent label keys, absent/empty In values, and nonempty values on Exists/DoesNotExist. Preserve controls for all valid operators, AND composition and empty selectors.

Add a focused namespace-gate test using the existing real Gateway/route fixtures and committed Namespace labels: the invalid NotIn selector must reject the cross-namespace route. No network environment or widened production visibility is needed.
''',
'ARCH-105': '''
# ARCH-105: Bind local credential-manager results to the source snapshot that produced them

## Current ownership

CredentialSourceRegistry stores desired sources and manager results under one RegistryState lock, but results are keyed only by logical namespace/name. CredentialManagerResult carries observed_generation without a source UID or input-revision binding.

forget_source removes the desired source and result together. However, replace_result unconditionally inserts a result without checking whether its producing source still exists. result returns it by logical key alone.

The asynchronous credential manager captures DesiredCredentialSource snapshots and retains separate SourceState. publish_manager_result constructs a result using the supplied source's generation and unconditionally calls replace_result. Its source_is_current and consumer_is_current condition gates memory cleanup only; a false condition does not prevent the local result write.

## Concrete lifecycle witnesses

First, a manager tick can capture source A while the serial LinkSys worker later deletes A and completes forget_source. When the captured tick reaches publish_manager_result, source_is_current is false, but replace_result restores the deleted source's result. LinkSys status processing can then find no raw resource and skip processing without repeating forget_source. A subsequent manager tick removes its private state for the absent source but does not remove this restored registry result. This is orphan local bookkeeping, without a claim of an unbounded leak or stale token publication.

Second, LinkSysHandler::parse removes a previous desired source and registers a newly eligible source. It removes the previous result only when no new source was registered. A same-name replacement with a different UID and the same metadata.generation can therefore retain the old result. LinkSysHandler::update_status filters only observed_generation and can project the old source's credential state, publication identifier, refresh times and counts onto the replacement resource.

There is also an actual producer-side pairing hazard: JobResult::Cleaned looks up the current desired source and the existing SourceState independently. A source replacement while cleanup is outstanding can pair the new source with old state. Adding a current-source check to replace_result alone would not prevent that old state from being stamped with the new source's generation.

## Bounded improvement

Give the existing private result slot a producing-source qualification: source UID, generation and the canonical input revision. Keep at most one result slot per logical key. The registry should compare the supplied qualification with its current desired source and perform insertion under the same RegistryState write lock. A deletion or replacement must therefore prevent an older producer from restoring or relabeling the local result.

The result read must also reject a slot whose qualification no longer matches the current desired source, including a same-name UID replacement. This avoids depending on every handler to clear results during the transient remove-and-register sequence.

Before constructing a result, publish_manager_result must verify that SourceState.identity and SourceState.input_revision belong to the supplied source. Preserve the existing source_input_revision algorithm as the single owner; if registry qualification needs that calculation, relocate or expose that existing private calculation at the narrow shared source-state boundary rather than duplicate its fingerprint logic. Compare metadata/proof inputs without cloning resolved TLS material.

These changes concern local result ownership. Retain existing worker-generation, reconciled-consumer, source-currentness, publication, Secret and Lease checks for their existing effects. No upstream I/O or external authority check belongs inside the registry lock.

## Required semantics and limits

Preserve one serial resource worker, key-only requeues, changed-result requeue suppression, current-source expiry reporting, desired/published/conflicted counts, persist-off fan-out counting, retries and last-good retained publication policy. A failed current acquisition may still report the current state's retained publication as before.

Private result qualifications must not become operator schema or conf_sync fields, and must not expose credentials or resolved TLS material through Debug, serialization or logs. Source snapshots, RetainedPublication, memory entries and request-pinned credentials keep their separate accepted ownership.

This does not establish atomic external authorization revocation, exactly-once publication, physical erasure, or a Gateway token defect. ARCH-094's service/gauge lock scope is a separate cause.

## Original knowledge and deduplication

The LinkSys architecture and credential-source-single-active-memory guard establish UID-qualified active source identity while accepting independent retry and runtime owners. credential-copy-retention.md does not authorize result metadata to outlive its source without qualification.

The pending resource-reference-contract specification and implementation explicitly retain scoped asynchronous owner/proof checks and exclude broad cross-system atomic revocation guarantees. This recommendation stays within one local registry and its actual result producer.

## Future verification

Use deterministic source snapshots rather than scheduler timing:

1. Capture a registered source and valid current SourceState, call forget_source, then attempt its actual result publication; no result may be restored.
2. Store a result for UID A, register UID B under the same logical key and generation, and verify that B cannot read A's result.
3. Exercise a cleanup completion with old SourceState and a replacement desired source; no result may be attributed to the replacement.
4. Reject a SourceState input revision that differs from the producing source's canonical revision.

Retain the existing persist-off counting and monotonic expiry regressions. Their SourceState fixtures currently leave input_revision empty; qualify those fixtures with the real canonical revision rather than weakening production ownership checks for test convenience. Existing tests and source inspection are evidence only; no tests were run for this review.
'''
}
for ident, slug, kind, priority in [
 ('ARCH-099','target-local-policy-timestamps','architecture-debt','P3'),
 ('ARCH-100','namespace-selector-expression-cardinality','behavior-defect','P2'),
 ('ARCH-105','credential-result-source-ownership','behavior-defect','P3')]:
 record.candidate(ident,slug,'PART-403','/root/verify_ip_union',kind,priority,bodies[ident])

prefix='edgion-controller/src/conf_mgr/sync_runtime/resource_processor/'
dispositions={
'attached_route_tracker.rs':'No finding: actual producer records deduplicated accepted listener names; wildcard/explicit double counting falsified.',
'backend_tls_policy_ref.rs':'ARCH-099: full timestamp inventory clone couples target-local arbitration to unrelated policies.',
'context.rs':'No finding: typed shared state and trigger-chain checks feed existing coalesced workqueue without new I/O owner.',
'credential_source_state.rs':'ARCH-105: unqualified logical-key result slot admits orphan or replacement-source results.',
'frontend_ca_ref.rs':'No finding: typed Secret/ConfigMap keys reuse canonical bidirectional manager and preserve reverse requeues.',
'gateway_route_index.rs':'No finding: dependency index, accepted-listener accounting and published serving state have distinct roles; replacement requeues dropped dependencies and excludes secrets.',
'handler.rs':'No finding: synchronous finalization and asynchronous post-publication hooks serve distinct actual ownership checks.',
'listener_port_manager.rs':'No finding: replacement registration, deterministic peers and intra-Gateway scope verified; runtime reset retention explicit.',
'mod.rs':'No finding: actual exports/aliases/runtime consumers match ownership.',
'namespace_store.rs':'ARCH-100: invalid expression cardinality, including empty NotIn, accepted by namespace gate.'}
support={
'AGENTS.md':'Full original retained; only root applies.',
'skills/SKILL.md':'Full retained index.',
'skills/00-design-rules.md':'Full retained graded gates.',
'/Users/caohao/.codex/skills/edgion-architecture-review/SKILL.md':'Full retained workflow; current1-135 reread.',
'skills/04-review/SKILL.md':'Full retained index.',
'skills/04-review/knowledge-routing.md':'Full retained routing.',
'skills/01-architecture/01-controller/05-resource-processor.md':'Full retained source/derived/serial ownership.',
'skills/01-architecture/01-controller/06-requeue-mechanism.md':'Full.',
'skills/01-architecture/05-resources/SKILL.md':'Full.',
'skills/01-architecture/05-resources/01-gateway.md':'Full.',
'skills/01-architecture/05-resources/12-backend-tls-policy.md':'Full.',
'skills/01-architecture/05-resources/18-link-sys.md':'Full.',
'skills/02-features/03-resources/01-gateway.md':'Full; stale cross-Gateway listener prose noted.',
'skills/04-review/routing/SKILL.md':'Full.',
'skills/04-review/listeners/SKILL.md':'Full.',
'skills/04-review/handler-requeue/SKILL.md':'Full.',
'skills/04-review/handler-requeue/mandatory-design/handler-requeue-rule.md':'Full.',
'skills/04-review/handler-requeue/regression-guards/backend-tls-winner-single-snapshot.md':'Full.',
'skills/04-review/status/regression-guards/deterministic-condition-messages.md':'Full.',
'skills/04-review/security/intentional-design/credential-copy-retention.md':'Full.',
'skills/04-review/link-sys/regression-guards/credential-source-single-active-memory.md':'Full.',
'skills/04-review/controller/regression-guards/source-scoped-watch-snapshots.md':'Full retained.',
'skills/04-review/handler-requeue/regression-guards/controller-derived-fields-use-processed-cache.md':'Full retained.',
'tasks/pending/resource-reference-contract/01-spec.md':'90-107,768-776,905-918,1192-1202.',
'tasks/pending/resource-reference-contract/04-implementation.md':'288-311.',
'edgion-controller/src/conf_mgr/runtime.rs':'Full.',
'edgion-controller/src/conf_mgr/processor_registry.rs':'195-230.',
prefix+'ref_manager.rs':'Full.',
prefix+'processor.rs':'572-584,765-785,865-880,910-943 retained,956-978.',
prefix+'handlers/route_utils.rs':'325-367,469-568,640-699,747-825.',
prefix+'handlers/gateway.rs':'483-601,705-753,920-1020,1025-1058,1721-1785.',
prefix+'handlers/backend_tls_policy.rs':'255-286,380-489,545-596.',
prefix+'handlers/edgion_backend_traffic_policy.rs':'85-110,533-578.',
prefix+'handlers/link_sys.rs':'194-271,300-320,354-383.',
prefix+'handlers/edgion_plugins.rs':'235-256,387-407.',
prefix+'handlers/secret.rs':'1-82.',
prefix+'handlers/configmap.rs':'1-74.',
'edgion-controller/src/services/credential_manager/service.rs':'SourceState/job declarations;300-435,440-557,943-1040,1150-1188,1527-1658,1694-1749; lifecycle selections.',
'edgion-controller/tests/unit/services/credential_manager/service_test/tests_memory.rs':'1-150,350-440.',
'edgion-resources/src/resources/gateway.rs':'458-490.',
'edgion-resources/src/resources/refs/owned.rs':'96-145.',
'edgion-controller/src/conf_mgr/conf_center/kubernetes/controller.rs':'459-614.',
'config/crd/gateway-api/gateway-api-standard-v1.6.2.yaml':'1950-2005,2486-2618,4160-4205; Selector/from/CEL search.',
'Cargo.lock':'kube-core2.0.1/k8s-openapi0.26.1 pins.',
'/Users/caohao/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/kube-core-2.0.1/src/labels.rs':'1-365 implementation;365-430 tests.',
'edgion-controller/tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/route_utils_test.rs':'1-176,320-462.'}
for stem in ['attached_route_tracker','backend_tls_policy_ref','credential_source_state','gateway_route_index','listener_port_manager','namespace_store']:
 support['edgion-controller/tests/unit/conf_mgr/sync_runtime/resource_processor/'+stem+'_test.rs']='Full actual test bodies.'
record.partition('PART-403','/root/verify_ip_union',{prefix+k:v for k,v in dispositions.items()},support,list(bodies),[
'Static source review; no writes/builds/tests/application/Git/network by analyst; support zero primary credit.',
'Context-only route_utils344-346 accepts Selector with no selector; current test expects it but feature/schema require selector. Separate contract conflict excluded from100.',
'Gateway feature page cross-Gateway conflict prose is stale versus current implementation/tests/architecture intra-Gateway scope; no scope expansion.'])
print('PART-403 and ARCH099/100/105 persisted')
