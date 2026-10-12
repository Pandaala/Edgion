> **Historical, superseded merge checkpoint.** The 2026-10-09 merge pause and
> merge-specific no-commit instructions below no longer govern current work.
> See [00-context.md](00-context.md) for current authority and
> [07-progress.md](07-progress.md) for committed batches and remaining acceptance.
> The pre-merge R6 plan and residual inventory are preserved as historical evidence;
> their old pending labels are superseded by later reviewed commits.

# Reference contract merge checkpoint

The user paused implementation on 2026-10-09 to merge the tracking remote branch
and resolve conflicts. The user will commit the merge. Do not commit or push
this merge. Resume reference implementation only after the user completes that
checkpoint; revalidate all plans and evidence against the merged HEAD first.

## Accepted work and current boundary

Local HEAD before merge: `5cd67dd91fb5e0d8e52ed30d95f61af1aa21b8ea` on `feature-08-30`.
R5d WAF actual-owner references committed as `558433cf9`.
R5e remaining plugin ConfigData references committed as `5cd67dd91`.
R5e passed two independent final reviews, 433 distinct focused unit tests,
2 actual-owner compile-fail doctests, 18 tracing-guard tests and all 14 root
checks, including workspace check/Clippy with zero warnings. All 8926 old test
identities survived. This evidence applies to the pre-merge tree only.

R6a is PLAN ONLY, not approved and not implemented. No R6 production edits or
Cargo tests have started. Its plan and residual inventory are preserved below;
they must be independently reviewed again after merge. The exact 19-path proposed
allowlist and test inventory remain in tmp/resource-reference-contract/r6-plan-20261009/.

## Required remaining work

- R6: finite annotation, LinkSys, DNS, plugin attachment, log output and process
  instance references; retain the detailed residual dispositions below.
- R7: outbound Service references and related actual-owner boundaries, preserving
  native ExternalAuth versus Edgion plugin distinctions, canonical egress/TLS,
  deadlines, and existing permission policies.
- R8: native Gateway certificate/frontend CA, BackendTLSPolicy references and
  remaining TLS/ACME/output identity boundaries; preserve standard group/kind,
  role-specific partial/fail-closed behavior and existing UID/CAS/Lease authority.
- F1: whole-consumer coverage audit, repeated deep independent reviews of
  ReferenceGrant consistency, reference coverage and Gateway API conformance;
  documentation/schema/knowledge reconciliation, English/Chinese user docs,
  obsolete helper removal and task/index/inbound-link cleanup.
- F2: fresh workspace verification and real-environment dynamic integration,
  admission and Gateway API conformance with current binaries/images; preserve
  reports and exact cleanup of owned processes/resources. Historical unit or
  earlier live results are not current conformance acceptance.
- Keep the original tasks/todo/core-api-group-alias-contract.md open until the
  complete authorized task is accepted. Do not lose remaining work during merge.

After the user commits the merge, return to the authorized serial IMPL workflow:
plan, independent review, implement, repeated final review, checks, local commit
per accepted batch, no push. Merge-specific no-commit authority takes precedence
until the user completes the merge checkpoint.

## Preservation and merge requirements

Do not overwrite unrelated review-results/. Preserve the existing
uncommitted 07-progress.md continuation and this checkpoint across the merge.
Resolve each conflict semantically against both sides and relevant caller/tests;
do not select ours/theirs wholesale. Remote changes may supersede inventory
findings; verify current requirements and behavior rather than preserving old
implementation shapes mechanically. Do not start R6 code work during merge.

## Proposed R6a plan (pre-merge; not approved)

# R6a: Controller DNS annotation authority and generated-slice identity

PLAN ONLY at 5cd67dd91fb5e0d8e52ed30d95f61af1aa21b8ea. No tracked edits or Cargo. R6 is broader than this batch; see RESIDUALS.md. Read original root AGENTS (no nested AGENTS found), skills index/graded rules/resource index/review routing, task 01/02/04 serial unit 4, Service/EndpointSlice architecture, HTTPDNS prepared-runtime and EndpointSlice ownership-transition knowledge. Revalidated both earlier DNS inventories against this HEAD.

1. **Concrete defects and authority.** The processed Service handler strips denied DNS annotations, but the independent Kubernetes producer lists original Services and uses their annotations before a LinkSys GET and resolution. It never checks that Service's Grant. Task 01 section 5 requires actual field identity before authorization/read; task 02 explicitly lists core Service and Gateway DNS annotation Grant policy. Separately, producer skip state projects observed slices to only namespace/service-name: it ignores Service UID and collapses IPv4/IPv6 presence. Equal content after Service recreation, or removal of one of two slices, can skip required repair. Task 04 serial unit 4 requires exact existing Service-UID output ownership and forced SSA. Eventual Kubernetes GC can heal the UID case; permanent staleness is not claimed.

2. **One finite grammar, actual field loans.** Add resources/refs/dns_fields.rs, with borrowed explicit DNS LinkSys key parsing and finite constructors from actual Service/Gateway annotation owners. Preserve current LinkSysKey::parse exactly: outer/component trim, one slash, nonempty halves, no bare-name default and no DNS-label tightening. Add only a private fixed-LinkSys identity constructor alongside the existing private template grammar exception; no public arbitrary identity/profile constructor. Actual owner source remains validated core Service or Gateway API Gateway with omitted namespace -> default, and raw metadata remains the dependency-owner identity. The field loan, not caller strings, selects source group/kind. Common LinkSysKey::parse delegates to this sole grammar and retains its owned representation/API for existing consumers. Process YAML uses the grammar without acquiring a fictitious resource source. Gateway/EGC cold field migration remains a named later R6 item; this batch does not claim complete DNS consumer migration.

3. **Controller gate.** Replace detached from_ns/from_group/from_kind/annotations authorization with actual Service/Gateway loans. Keep a small shared current-permission decision using typed GrantQuery and the existing runtime RefGrantStore/global toggle. Handler entry registers a valid cross-namespace dependency before checking policy, preserving raw ResourceRef and existing clear-before-add/delete. Producer uses that same decision without creating a second registry/queue or relying on processed cache. Absent/malformed annotation remains unusable and falls through to inline/default as today; it creates no edge/query/read. Invalid actual owner creates no query/edge/read and cannot ship a usable external annotation (Service strips it; Gateway follows its existing annotation-error strip/status branch). Same-namespace and validation-off exemptions occur only after the finite identity is formed. Keep existing Service warning ownership and Gateway status vocabulary; add no new metrics or logging framework.

4. **Raw producer admission.** Pass the already available Arc<ControllerRuntime> from Kubernetes center.start_leader_jobs into the DNS task/reconcile boundary. For a Service-selected reference, decide permission before LinkSys GET, preparation or selected HTTPDNS use. A denied Service annotation behaves like the processed stripped copy: try same-layer inline servers, then Controller default/System. Do not clone the Service or its annotations to simulate stripping. Preserve explicit process-default provenance: a configured administrative default may legitimately select the same LinkSys denied to the tenant annotation. Missing/wrong-provider selected LinkSys remains System without falling to lower precedence; an existing failed HTTPDNS generation retains its configured fallback and prepared failure reuse. No handler IO, new Grant API list, store, worker or queue. Existing raw LinkSys mTLS-material limitation remains.

5. **Generated output skip.** Keep deterministic IPv4/IPv6 names and forced SSA. Build a bounded observed index by actual slice namespace/name (borrowing the already listed records). Skip only when the stored successful content fingerprint matches and EVERY desired address-family slice exists with matching addressType, service-name label, and exact v1/Service owner name+UID from the current actual Service. Missing UID continues to skip publication. A mismatched UID or missing single family forces normal apply. Keep IP/port-only fingerprint semantics, full-success-only state updates, preparation state, resolution-error/empty-answer LKG, desired-Service pruning and existing GC behavior. No second mutable reverse index.

6. **Explicit async limit.** This batch adds no source/Grant/Lease revision fence and no extra Service GET before SSA. Authorization is taken before acquiring the selected LinkSys; an already admitted asynchronous operation retains normal completion semantics. No absolute mid-flight revocation guarantee is claimed. Service edit/delete/recreate during await, Grant revocation after admission, and GC list/delete races remain separately reviewable async-output boundaries. Also record (do not silently fix) that obsolete address-family slices are currently retained while their Service remains desired; removing a no-longer-desired family requires a separate deletion-authority design. The present family correction concerns REQUIRED-family presence, not obsolete-family deletion.

7. **Frozen tests and fixture shape.** Preserve all old IDs/assertions/ignores. Changed internal signatures may use appended mirrored-test-only wrappers for existing check_dns_resolver_ref_grant/reconcile_once/can_skip_apply call shapes: fixtures construct actual Service/Gateway owners and real desired/observed slice records, then invoke the production owning entry. No production compatibility shim, inline cfg(test) body, public visibility expansion or copied owner for runtime convenience. If preserving a specific old body proves impossible, return an exact call/setup amendment for independent review before editing it. New tests call production owning APIs directly, not those wrappers. No Gateway cold tracing symbol changes in this batch.

8. **Meaningful additions.** Resources: borrowed lifetime, exact whitespace/slash/nonempty grammar including presently accepted non-DNS spellings, actual Service versus Gateway Grant source, omitted/default versus explicit invalid owner, no generic profile. Common: existing resolver precedence/provider/fallback tests unchanged plus canonical grammar parity. Controller: Service/Gateway handlers retain valid missing/denied recovery and raw delete/retarget edges; wrong-source Grant/correct Grant/revoke; validation-off isolated exact subprocess with executed-test assertion; invalid/absent/malformed inputs have no edges or LinkSys reads. Mock kube producer: original denied annotation cannot GET LinkSys, effective spec is inline/default/System and no tenant HTTPDNS handle is selected; valid/correct Grant fetches, revoke clears prepared selection on the next tick; process default remains independent. Preserve provider failure-reuse tests. Output: matching UID/all families skips, changed UID with same content applies, either family missing applies, wrong name/label/addressType/owner fails skip, partial SSA failure does not advance fingerprint, resolution failure keeps LKG, successful apply payload retains forced SSA and owner UID. Use existing tower mock Client and literal IP/System paths, no network or new resolver framework. Zero selected HTTPDNS use follows effective-spec assertion plus unchanged dispatch, not a fabricated counted transport claim.

9. **Documentation and verification.** Update exact task spec/manifest Controller subset and owning Service architecture/HTTPDNS KB, explicitly retaining later Gateway cold work and async limits. After independent PASS + EXECUTE: capture originals/full old IDs and fresh affected crate lib listings; preserve raw failures; run resource DNS loans/docs, Common DNS selection/preparation, Controller DNS service/builder/guard/Service/Gateway owning suites and relevant Gateway unchanged DNS regression suites. cargo fmt --all before freeze. Parent owns workspace gates, two final reviews and local commit. No staging/commit/push here; no new schemas, Secret transport, egress implementation, request parsing/locks/allocations, UID-wide retention policy or generic reference engine.

## Review decisions requested

- Accept finite existing string grammar via a private fixed-LinkSys parsed identity (rather than silently DNS-validating legacy field spellings).
- Accept the bounded Controller-only sub-batch and explicit normal-completion/no-new-async-fence limit.
- Accept desired-family/Service-UID skip repair while obsolete-family cleanup remains separately recorded.

Exact allowlist: 19 paths in allowlist.json (including one new production module and its mirrored tests); root owns 07 exclusively. This is an upper bound, not a requirement to manufacture changes in every test file.

## Complete R6 residual disposition (pre-merge)

# R6 finite residual disposition at 5cd67dd91

This is a coverage ledger, not a claim that all remaining fields need code changes. Original task 01/02 and serial unit 4 remain the authority. Supporting read-only evidence: r6-service-dns-inventory-20261008.md, r6-service-dns-test-inventory-20261008.md, r6-other-consumers-inventory-20261009.md; current DNS producer/handler/parser/startup call chains independently rechecked.

| Family | Current evidence | Disposition |
| --- | --- | --- |
| Service/Gateway DNS annotation Controller authority; raw Service DNS producer | Detached handler tuple; producer bypasses processed stripping | R6a proposed actual loans/shared runtime permission, no new async fence |
| DNS-produced EndpointSlice identity | Exact UID in builder, but skip drops UID and family existence | R6a proposed desired-family/UID skip repair; forced SSA/LKG unchanged |
| DNS asynchronous output freshness and obsolete-family deletion | No Service/Grant revision fence across await; GC deletes from earlier list; still-desired Service retains obsolete family | Explicit later output-authority review, not silently fixed or claimed safe |
| Gateway Service/Gateway/EGC DNS cold consumers | Existing grammar/ResolverSpec and prepared provider; Service candidate still reconstructed on request path | Later R6 DNS cold-field migration; no added hot-path work in R6a |
| Controller process DNS default | Separate explicit string provenance and kube-less System fallback | R6a parser delegation preserves behavior; no invented Service source |
| EGC globalPluginsRef/Gateway HTTP plugin annotation | R4d canonical reads and ambiguity denial already protected; EGC collector still raw format/add_ref | Later owning attachment/valid-edge migration; preserve missing-global drop, ordered globals and cluster authority |
| Gateway and L4 stream annotations | Gateway local bare name/slash rejection differs from TCP/TLS/UDP slash-retaining grammar | Later finite owning adapters; no new Grant/registration policy |
| Gateway ProxyProtocol resourceRef | Prior owning-adapter residual; existing schema/visibility/fallback | Later bounded Gateway scope, not DNS or generic Selector work |
| DSL/Redis/Webhook/AI LinkSys refs | Actual runtime typed provider authority; detached fixed keys remain | Later finite plugin loans; preserve aliases versus object names and Redis counter namespace, explicit existing grammars/provider failure policies |
| Template Secret refs | Already actual PreparedPlugin/LinkSys references and canonical credential pipeline | Counterevidence: retain existing implementation/tests; final audit only unless a concrete missed caller appears |
| Log output/fallback ES/Kafka/OTLP | output::LinkSysRef already DNS-valid and explicit namespace; cold sender binding remains separate | Later cold owning sink binding; no new Grant, per-send validation, disabled/fallback/SSL behavior changes |
| GatewayClass parameters | R5a actual native owner contract implemented | No duplicate migration |
| Dedicated Gateway Service ref+UID | Exact ref and UID-derived name/provisioner contract already protected | Preserve; later shared instance identity audit with process locator |
| Process EDGION_GATEWAY_SCOPE/UID | Separate process locator with current local grammar | Later finite instance binding; never fabricate a Kubernetes owner |
| EndpointSlice Service associations | Gateway joint old/new owner publication and raw Controller wakes already protected by earlier batches | Preserve existing different malformed/fallback policies; final finite adapter audit remains, distinct from DNS producer skip |
| ACME/generated credentials/output destinations | Earlier material/publication work has role-specific UID/CAS/Lease fences | Final output coverage audit; no generic writer resolver, no new authority inferred here |

R6a completion will not close R6, R7/R8, F1 cleanup or live verification. Root owns overall ledger and sequencing.

## Proposed R6a exact allowlist (revalidate after merge)

```json
{
  "baseline_head": "5cd67dd91fb5e0d8e52ed30d95f61af1aa21b8ea",
  "paths": [
    "edgion-resources/src/resources/refs/mod.rs",
    "edgion-resources/src/resources/refs/identity.rs",
    "edgion-resources/src/resources/refs/dns_fields.rs",
    "edgion-resources/tests/unit/resources/refs/dns_fields_test.rs",
    "edgion-common/src/dns/resolver_select.rs",
    "edgion-common/tests/unit/dns/resolver_select_test.rs",
    "edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/dns_resolver_guard.rs",
    "edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/service.rs",
    "edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/gateway.rs",
    "edgion-controller/src/conf_mgr/conf_center/kubernetes/center.rs",
    "edgion-controller/src/services/dns_resolver/service.rs",
    "edgion-controller/tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/dns_resolver_guard_test.rs",
    "edgion-controller/tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/service_test.rs",
    "edgion-controller/tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/gateway_test.rs",
    "edgion-controller/tests/unit/services/dns_resolver/service_test.rs",
    "tasks/pending/resource-reference-contract/01-spec.md",
    "tasks/pending/resource-reference-contract/02-field-manifest.md",
    "skills/01-architecture/05-resources/16-service-endpoints.md",
    "skills/04-review/link-sys/regression-guards/httpdns-prepared-runtime.md"
  ]
}
```

# Existing source test inventory

No Cargo run in plan phase. Fresh compiled listings follow EXECUTE. Last committed Resources/Controller/Gateway counts: 1949/1794/5196; Common fresh count pending.

| Mirrored path | Existing test functions | Ignore attributes |
| --- | ---: | ---: |
| edgion-resources/tests/unit/resources/refs/dns_fields_test.rs | 0 | 0 |
| edgion-common/tests/unit/dns/resolver_select_test.rs | 13 | 0 |
| edgion-controller/tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/dns_resolver_guard_test.rs | 4 | 0 |
| edgion-controller/tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/service_test.rs | 2 | 0 |
| edgion-controller/tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/gateway_test.rs | 59 | 0 |
| edgion-controller/tests/unit/services/dns_resolver/service_test.rs | 11 | 0 |
