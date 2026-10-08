# Resource reference implementation plan

## Objective and gate

Implement the complete finite reference manifest; a shared helper alone does not
complete this task. Independent Astra plan review gates production edits.
Approved choices are in [00-context.md](00-context.md); exact operator fields and
exclusions are in [02-field-manifest.md](02-field-manifest.md). Candidate supporting
files are frozen in [06-subtasks.md](06-subtasks.md). One serial mutation lane.

## Architecture

Create resources/refs as the runtime-independent owner of borrowed GroupKind,
ObjectIdentity, explicit ResourceScope, field defaults, target validation and exact
Grant queries/matching. Private constructors, statically selected typed owning field
adapters, no public profile/allow_alias or arbitrary shared-wire reference binder.
Reuse existing Kubernetes name validators and Gateway API constants. No new dependency,
I/O, stores, locks, secret material or runtime policy in the pure module.

Keep authored wire, pure validated field capabilities and Controller authorization/
lookup distinct. Capabilities bind source+target+field provenance, not separately
overridable source strings. Dependency owner may differ from authorization source.
Owning traversal exposes stable typed field positions; selecting a slot never selects
an arbitrary contract. Expanded ConfigData condition placeholders bind to EdgionPlugins
after expansion. Snapshot identity data only when mutable reconciliation needs it;
request paths borrow precompiled targets/static defaults.

## Serial unit 1: pure/wire/input contract

- Add refs identity/contracts/targets/grants/owning adapters and move parent/status
  and ExtensionRef target semantics under this owner; update all imports, remove
  duplicate group normalization and legacy aliases. Retain section/port/controller.
- Standard comparison is exact. Approved custom lowercase core conversion happens
  once before validation/authorization/dependencies/lookup/runtime. Unknown/case/
  whitespace spellings do not alias. Strict fields remain strict.
- Split required standard local CA from optional custom cross-namespace CA wire.
  Preserve required ExtensionRef/CA/Grant/policy/parameters group/kind/name and
  explicit empty serialization. Mirror/custom CA/Webhook/custom policy optional
  group becomes presence-aware Option<String>. Custom CA kind remains required.
- Update only affected custom CRD interiors/defaults/required/enum/nullability.
  Vendored Gateway API CRDs stay authority; add no standard spec fields.
- Reuse scoped transform generator infrastructure to generate the narrow reference
  contract from pinned v1.6.2 Standard/Experimental bundles: exact source GVK/profile,
  defaults/nullability/bounds/unions/reference CEL and list inventory.
- Same temporary scoped validation view before Admin create/update/validate and
  existing FS/etcd source-adapter pre-schema checks. Prune optional nonnullable null
  within this scope, apply schema defaults, reject required missing/null-without-
  default. Keep authored document and unrelated checks. Preserve parent list
  section/port channel differences; no dedup or generic admission/discovery engine.
  Do not change supportedFeatures/version claims.

## Serial unit 2: material and authority

- Replace raw Secret reference+caller source strings in pipeline/CA helper with
  owning field capabilities. Introduce typed invalid-reference outcomes and handle
  all parse/readonly callers. Invalid identity: no fabricated edge/no lookup.
  Valid missing/denied: dependency registration before authorization/read.
- Migrate finite plugin/condition/LinkSys/AI, Gateway and EGC, strict EdgionTls/ACME
  reads; clear previous candidate material/edges before replacement.
- Pure exact Grant matcher; ReferenceGrant delegates it. Concrete target names,
  exact source/target namespace, Grant namespace binding, within-one-Grant from AND
  to, OR whole Grants. Only omitted to.name wildcard; reject explicit empty.
- Permission and authorization revision use the same canonical bound query.
  Obtain authorized matching-Grant revision from ONE store snapshot, rather than
  checking permission and later capturing an empty revision after removal.
  Cross-namespace acquisition/publication must fail closed without a current
  authorizer; same-namespace exemptions stay explicit. Migrate all five production
  revision callers and service publication rechecks; preserve Grant UID/RV-set,
  source/input/binding/publication/leadership/expiry fences.
- Preserve the existing global Grant toggle scope. Credential and Injector paths
  remain always enforced. Identity validation and dependencies never bypassed.
- EGC exact CA/client selection uses true Cluster source, explicit namespace and
  its approved administrative authority; do not manufacture Grant from.namespace.
- LinkSys CA remains Secret-only with resolver type whitelist/provider-specific
  unsupported-kind behavior. Health TLS remains policy-local regardless of Grant;
  no new checker/provider target support. Plugin/global CA keeps existing Secret/CM.
- Wire ForwardAuth local TLS through shared helpers with parse and readonly status,
  valid/missing/denied/invalid/update/removal and published runtime coverage.
- Preserve separate whole-generation EdgionTls/DSL/WAF and provider-specific LinkSys
  retained-generation behavior. Standard Gateway material denial prevents new use
  and global fallback. Do not invent universal revocation of old external handles.

## Serial unit 3: routes, filters, policies

- Canonical adapters for HTTP/gRPC/TCP/TLS/UDP backendRefs, native HTTP mirror,
  parent/type/parameters/policy fields. Compile supported actual target and strategy
  before runtime use; no raw kind reconstruction bypass.
- Reject standard core, unknown groups/kinds, pseudo-kinds and unsupported
  ServiceImport before same-name Service access. Cover backends/mod.rs and direct
  gRPC pg_upstream_peer independently of ordinary route compilation.
- Invalid weighted slots retain weights/rejection share. Mirror failure skips only
  mirror. Preserve ExtensionRef union/preflight/rule/backend scope, delegated child
  locality/reverse consent/dependencies/root requeue ownership.
- Wildcard delegation is a sealed NamespaceRoutes selector before object validation,
  never a fictitious '*' object/store/Grant/policy identity.
- Canonical dependencies/policy keys/scoped publication/AI logical slots; preserve
  slot occurrence and dynamic selector namespace wildcard separately.
- Inline ForwardAuth/Guardrail/AiGuard Service omitted namespace uses owning plugin
  context at construction; existing LinkSys owner defaults and explicit namespace
  remain. Reuse outbound client/TLS/egress; no new request-time lookup/parsing/locks.

## Serial unit 4: fixed refs, selectors, outputs

- Migrate every finite manifest fixed ConfigData/LinkSys/DNS/global-plugin/template/
  log-output/GatewayClass/instance/annotation consumer to typed canonical identities,
  preserving exact grammar/locality/visibility/provider/failure policy.
- Keep process DNS string vs resource DNS object and kube-less fallback, DSL alias
  vs object/counter namespace, exact annotation separators/L4 slash handling,
  global-plugin default namespace and explicit log-output namespace distinct.
- Dedicated Gateway namespace/name+UID and provisioner derivation stay equivalent.
  EndpointSlice association relabel/removal rebuilding and separate malformed
  fallback policies remain; Namespace labels/dynamic wildcards remain selectors.
- ACME/DNS/generated credentials use separate output/destination identity and
  existing operation authority. Preserve namespace/default/name, Lease/source
  cancellation, CAS/UID/RV/forced SSA, expiry/persist-mode and reader/writer owner
  fences. No handler network I/O or universal UID/write resolver.
- Audit actual lookup sites beyond type-name search. Excluded metadata/bootstrap/
  synthetic/request identifiers gain no object or Grant semantics.

## Verification and independent review

Each semantic unit updates schema, Controller, Gateway, status, docs and tests
together. Test the spec section 7 matrix in mirrored tests/unit; preserve meaningful
existing tests/omission fixtures. Assert no wrong-target lookup or fabricated edges,
valid missing/denied dependency recovery, canonical permission+revision agreement,
Grant race fail-close and valid siblings/weighted failures. Add generator drift and
custom CRD parity tests. Review actual diffs, not snapshot refills blindly.

Final checks: cargo fmt --all; workspace all-targets check/clippy; all nine AGENTS
gates; custom CRD coverage; no-tracing and English-only guards; focused affected
Resources/Controller/Gateway tests. Do not run cargo test --all without explicit user
request. Dynamic scenarios use existing single shared integration topology and event
waits. Live admission/conformance requires available environment and exact installed
artifact/channel/GVK/revision/toggle/profile/report evidence; unavailable execution
is a reported limitation, never an inferred pass.

After complete implementation obtain a fresh independent Astra deep diff review,
then another independent pass covering authority/revision races, manifest coverage,
admission/runtime/conformance and performance/retention boundaries. Reviewers read
original rules/knowledge personally. Fix blockers, rerun affected checks and
re-review corrections until all blocking findings resolved. Review count alone
never establishes correctness. Material deviations require updated plan review.

Publish concise default/alias user guidance and owning architecture/unification/
review knowledge. Narrowly repair the known stale documentation reference.
Delete only the closed source issue after implementation/review/gates; retain this
multi-document record. No commit, push or deployment.

## Hard-rule gate

Preserve DAG, pure leaf ownership, one canonical implementation, existing outbound
TLS/HTTP/egress mechanisms, precise authority, binary-owned logging/canonical access
log, secret redaction+schemars(skip), one serial queue/worker/requeue, clear-before-
add, no handler I/O, no hot-path allocations/parsing/locks/tracing, bounded memory,
Rust safety, English-only. No rustls/DNS-01/MCP/A2A/new Gateway API feature or unrelated
cleanup. Inspect relevant MUST rows against actual final diff.

## Progress

Independent plan review: PASS by ref_impl_plan_review (gpt-6-astra, xhigh).
The reviewer independently traced original rules and actual call chains; no
remaining blockers after the concrete construction/final-consumer amendment.
Production implementation: pure/wire/source foundation and multiple serial unit 2
credential consumers implemented; the finite manifest remains open. Final code
review pending.

## Concrete construction and final-consumer amendment

Owning reference visitors, not a public owner-plus-wire builder, mint capabilities:
EdgionPlugins/LinkSys/EdgionBackend/Gateway/EGC/EdgionTls/ACME and policy adapters
traverse their actual finite fields. A private FieldSlot enum identifies stage,
entry, nested condition position, TLS component and reference ordinal. The public
consumer sees an opaque slot token and validated capability. Where mutation requires
it, a cold prepared-reference snapshot retains owned identity data and private slot
mapping; consumers retrieve minted capabilities by tokens produced by the same
traversal. No constructor accepts caller source strings or arbitrary profile.
Readonly status uses the same visitor/preparation path without dependency writes.
Any simpler concrete API must retain this provenance guarantee and receive review.

Before passing outbound material to Common, owning resource compilation prepares
an opaque validated TLS input from the actual field references and resolved material.
Validate every ref group/kind/name/namespace and one-to-one material binding,
including ConfigMap-origin synthetic CA metadata, client Secret identity and
ordered CA slots. Invalid/missing/mismatched input fails closed. The Common
canonical resolver consumes that prepared input with its existing global snapshot;
it does not infer owner namespace from resolved Secret metadata or accept an
unchecked resource-wire local block. Inherited EGC TLS is validated with true Cluster
field authority/explicit namespaces before publication and again at the consuming
boundary. Retain direct programmatic already-resolved/no-operator-ref callers through
an explicit separate input path; no such path can carry unchecked operator refs.
Reuse existing TLS resolution, verify/SAN/client inheritance and provider restrictions.

Add exact scope support for the already registered HistoricalGrpc profile from
grpcroute-v1alpha2-v1.1.1-experimental.yaml, alongside the pinned v1.6.2 channels.
Generate/check its selected reference fragments and source-version parity fixtures;
never accidentally apply current-channel rules to the historical source adapter.

## Concrete owning binding API refinement (review gate)

Use one cold `PreparedPluginReferences::from_plugins(&EdgionPlugins)` snapshot
per credential pass. `bind`/`bind_mut` take the actual typed owner, compare its
namespace/name/UID/generation and exact authored fields in each private slot,
and return a borrowing wrapper. Do not compare resolved material or serialize
whole plugin configs. The wrapper lends actual entry `EdgionPlugin` config plus
its prepared `EntryReferences` through stage-specific iterators; callers match
the existing enum to run their material parser. This replaces a separate public
binding struct/enum variant per plugin without widening construction authority.
No arbitrary owner+wire binder, public position/index lookup or caller-selected
profile is exposed. A token borrows a private PreparedSlot from the same snapshot.

`EntryReferences` exposes typed finite getters/iterators for its real credential
Secret groups, single Secret roles, CA slots, client certificate and CredentialInjector
LinkSys branch. Group iterators retain authored group names, ordering and first-wins
behavior; the actual private position includes stage/entry/group/reference. Unsupported
plugin placement yields no consumable token. CA/Secret/LinkSys token access cannot
mint a target that was absent from the actual config or switch its contract.

Existing resolver functions receive the bound wrapper instead of an unchecked EP
plus source strings. A stage iterator yields mutable config and immutable entry
references together. Readonly status uses the identical typed binding. Prepare
condition credentials after ConfigData expansion, through a separate owning
condition visitor that retains actual fixed ConditionSet/root positions and RAP
profile keys. Auth material resolution and condition expansion keep their existing
ordering; do not reuse a snapshot across expansion that changes its authored slots.

Apply the same cold prepared/bound owner pattern to LinkSys/EdgionBackend/Gateway/
EGC/EdgionTls/ACME and policy adapters where disjoint material mutation requires it.
Immutable callers may directly borrow their owning visitor. Capabilities preserve
source, validated target, field contract and position; material and runtime policy
remain outside Resources. This refinement requires independent plan PASS before
implementing the refined owning binding API.

### Unit 1 evidence (in progress)

Shared refs foundation tests: 17 passed before subsequent target/bounds additions.
Controller SchemaValidator tests: 13 passed, including arbitrary TLS options key
redaction. Workspace all-targets check passed after CA/optional-group/source-view
migration, before later target compilation edits. These are scoped results, not
final verification. Source-contract Astra scoped review PASS after correcting
canonical/beta offline rejection and diagnostics; subsequent bounded-projection
refinement still requires fresh verification. Owning adapters are partly
implemented; the full finite manifest remains open.

### Borrow-lifetime correction for the refinement

A stage iterator yields opaque `BoundPluginEntryMut`, never a public tuple of
`&mut EdgionPlugin` plus snapshot-lifetime tokens. The entry retains the mutable
config privately; `config(&self)`, `config_mut(&mut self)` and
`references(&self) -> Result<EntryReferences<'_>, ReferenceError>` provide access.
Every references/token/validated-target/query getter loans its output for the
current shared borrow of the bound entry, never the longer prepared snapshot
lifetime. Recheck the entry's exact current authored slots before each loan.
Consequently the caller cannot mutate, replace or drop the bound config while a
capability/derived borrowed identity is live; a new loan after changing an authored
field fails. Resolved-only mutation preserves the snapshot match. Compute owned
material under an immutable config/reference loan, end it, then write resolved
fields through `config_mut`. Do not expose the snapshot token lifetime via any
public accessor, Clone or into_parts. Borrowed tokens may be Copy only without
widening the active owner loan. Async persisted authority uses an explicitly owned
frozen binding with existing UID/generation/Grant revision rechecks.

Refined owning binding plan: PASS by ref_binding_plan_refinement (Astra xhigh).
Implementation gates: loan lifetime compile-fail tests; immutable entry enable;
unsupported stages still clear/count but mint no token; exact discriminant,
role/presence/group order/labels/ref topology checks; invalid results per slot;
condition preparation after expansion; preserve HMAC metadata/provider differences.
Fresh Resources resource unit tests: 1,543 passed. Shared refs tests: 20 passed.
Controller schema tests: 13 passed. These remain intermediate checks.

### Owning resolution progress

Plugin binding loans, concrete owner identity, and LinkSys provider loans are
implemented. Plugin snapshots preserve enablement and HeaderCertAuth mode;
disabled entries mint no material tokens. LinkSys preserves disabled TLS and
provider-specific unsupported CA selection without querying a different kind.
Bound Secret fetching validates before dependency registration, checks the actual
dependency owner, and captures the Secret plus an authorized Grant revision in
one lookup. The bound authorizer returns no revision after revocation.
CSRF, BasicAuth, KeyAuth, JWT, JWE and HeaderCertAuth are connected to these loans;
their status classifiers inspect actual fields rather than only dependency edges.
LinkSys auth/TLS/bootstrap resolution is being connected. Async frozen bindings,
remaining material families and final Common consumption remain unfinished.

Intermediate verification: owning refs tests 30 passed; loan compile-fail doctests
4 passed; bound Secret pipeline tests 12 passed; plugin resolver tests 29 passed
before subsequent HeaderCertAuth/LinkSys edits. No final-review or completion claim.

### Async frozen-binding refinement (approved)

`FrozenFieldReference` is opaque and created only by a validated field token.
Prepared owning snapshots use one private typed Arc containing metadata and the
full authored reference topology, with no material. Frozen inputs share that Arc
and retain a private finite slot; they do not duplicate an entire topology per
input. Their borrowed Grant queries use captured concrete identities. Wrong-owner
variants fail; currentness compares the full actual owner topology and metadata.

At acquisition, retry, publication and completion acceptance, read the actual
owner through the existing processor registry RawStore. Absent processor/resource
or failed downcast denies the operation; fixtures install real owning resources.
Re-run synchronous owner/registry/Grant checks after awaited leader, DNS and issuer
steps immediately before the next side effect. Require authorized revision
`Some(expected)`; revoked or missing authority cannot compare equal to an empty
revision. Include frozen semantic bindings in source/consumer registry equality,
input revisions and asynchronous completion acceptance. Compare a shared owner
topology once per distinct snapshot per recheck, then each slot/Grant/Secret UID/RV.
Preserve existing input, generation, leader, expiry, CAS and SSA fences. This adds
no I/O, store or cross-system atomic-revocation claim.

Independent concrete plan review: PASS by ref_binding_plan_refinement (Astra
xhigh), after explicit shared-topology and post-await fail-closed corrections.

### Async cleanup and recovery refinement

The existing cleanup helper awaits a Secret GET after its callers check the
leader fence. Add a required async authorization callback after exact old
ownership/marker checks and UID/RV precondition construction, immediately before
DELETE. Callback uncertainty or stale worker generation returns a fixed
StaleOperation error so the existing error path retains the cleanup expectation;
an obsolete desired-binding eligibility check returns false. Callbacks await the
leader check first, then synchronously evaluate generation and eligibility.
Startup cleanup recomputes desired publication identity at this final boundary.
Leftover cleanup validates the source and rejects deleting a current persisted
binding that still needs this publication. Removed-source cleanup retains its
exact old metadata authority without requiring a live resource or Grant.

Recovery first gathers valid old owned output DeleteExpectations using their
actual binding/generation metadata, filtered by target, consumer UID and source
UID/logical key. Invalid current references, input revisions, contents, expiry or
publication disagreement reject credential material while retaining all such
cleanup metadata. Persist-off never adopts recovered token material. A mode
transition only reuses retained material if the frozen input revision is equal.
The already-scoped non_kube_common startup seam rechecks optional etcd leadership
after awaited readiness before starting the memory worker.

Current intermediate evidence: 46 credential-manager tests pass, including real
RawStore owner changes/deletion, Secret RV changes and exact Grant revocation.
Owning refs received a scoped Astra PASS; overall final review remains pending.

Independent refinement review: PASS by ref_async_cleanup_plan_review (Astra
xhigh), with explicit obsolete-versus-retry and material-versus-cleanup boundaries.

Implementation now captures FrozenFieldReference in bootstrap/TLS inputs and
Injector consumers. Source and consumer currentness read the real typed RawStore
owners and fail closed on absence/downcast failure; captured Grant and Secret
versions are rechecked. Revisions and completion acceptance include the frozen
bindings. All six publication fences run before the final synchronous check.
Injector Secret/LinkSys/output resolution uses owning loans and clear-before-set.
Source refs: 36 focused tests pass; borrowing boundaries: four compile-fail
doctests pass. The 46 manager tests pass after recovery gating, including retained
cleanup metadata on authority loss. Post-GET cleanup callback tests are pending
their serial run. This is intermediate evidence, not full manifest closure.

Independent scoped implementation review ref_async_impl_review_r1 (Astra xhigh)
found four concrete defects: notification epochs invalidated the emitting job,
stale completed writes lost cleanup metadata, LinkSys denial was omitted from
Injector status, and AuthorizedSecret derived Debug exposed raw material.
Corrections separate epoch-free binding completion from retry scheduling, retain
cleanup-only expectations from stale/partial results, scope pending-status
deduplication to Secret dependencies, and use fixed redacted Debug. The reviewer
rechecked the corrections and returned scoped PASS. Fresh manager50 and
Injector/handler16 tests pass; bound pipeline13 including nested/alternate Debug
also passes. Overall coverage/final review remains open.

### CA material provenance and replay refinement

Independent concrete plan review ref_async_impl_review_r1 (Astra xhigh): PASS.
Both ConfigMap-to-Secret producers will use one Controller wrapper accepting
the actual fetched ConfigMap, preserving name, namespace, UID and RV, and
stamping the reserved `edgion.io/controller-ca-source: ConfigMap` annotation.
Ordinary Secret embedding paths remove that annotation. It expresses source
kind inside the existing trusted resolved transport, not cryptographic authority.
Common owning proof checks ordered refs against exact material kind, name and
namespace; ConfigMap material requires the exact marker, while Secret/client
material rejects any marker. Clear-before-resolve remains the trust boundary.
EGC retains its real cluster owner and explicit namespaces before aggregation;
global CA/client inheritance validates each independent axis. Disk bootstrap
uses an explicit no-reference interface instead of a fabricated reference.

The canonical TLS material fingerprint includes name, namespace and this marker,
but continues excluding UID/RV. Incoming owning proof must run before persistent
runner, Webhook and HTTPDNS equality/RV shortcuts: an unchanged RV and PEM must
not turn an invalid new reference into successful reuse. Preserve accepted old
generation retention and HTTPDNS unusable/fallback behavior. Regression coverage
includes same-name different-kind material, absent/unknown/forged markers, wrong
namespace, missing/extra/reordered material, and invalid same-RV replay. Update
reserved-annotation documentation and the shared fingerprint knowledge page.

Scoped CA/plugin implementation review ref_async_impl_review_r1 (Astra xhigh)
returned PASS for wrapper/sanitizer/fingerprint, OIDC/LDAP/ExtProc and Wasm owning
migrations. Fresh CA-helper7, Common fingerprint2 and EP-handler100 tests pass.
Handler regressions cover forged fields on inactive/wrong placements, same-name
ordered Secret/ConfigMap material, exact ConfigMap grant deny/allow/revoke with
recovery edges, and Wasm auth/pull/TLS activity across all four stages. The
unsupported final-response Wasm placement clears material without acquisition.
The historical missing-object-only Wasm classifier fixture now provides real
local material to retain its no-Grant-needed intent. Wasm status/annotation and
fingerprint knowledge is updated. Common final owning proof/replay and the rest
of the manifest remain unfinished; these are scoped intermediate results.

### Effective condition owning visitor refinement

Independent concrete plan review ref_async_impl_review_r1 (Astra xhigh): PASS,
with readonly identity/Grant/existence status required for both standalone and
condition HMAC group refs, without deeper YAML status validation. Matching also
records entry enable, container presence and RAP branch discriminants; sorted
profile keys remain stable under HashMap insertion-order changes. Higher-ranked
callbacks return unit and cannot release borrowed leaf/token values.

Add `refs/condition_fields.rs` with a synchronous `PreparedConditionReferences`
built from the actual EdgionPlugins after ConfigData/RAP expansion. Keep it
separate from the raw plugin topology: frozen Injector owner comparison must not
depend on effective RAP sidecars absent from RawStore. Extend the private token
loan with this synchronous topology; freezing an effective condition token is
rejected, never converted into a raw-owner frozen descriptor.

Use finite private root positions for all four stage entry skip/run sets,
request/final-response body skip/run sets, request/response dye rule skip/run
sets, request-stage RequestRestriction sets, and effective RAP profile/group/rule
IP variants and URL resolved candidates. Sort real RAP map keys; retain each
actual key. Condition leaf positions distinguish AllOf leaves, AnyOf direct
clauses and AnyOf nested AllOf leaves. Descriptor traversal records root presence,
container shape, leaf discriminants, SecretMatch key/reference wire, HMAC group
names/order/reference wires, and real EP metadata; it never clones a Condition,
resolved credential, IP list, matcher, certificate or config body.

Whole-owner bind checks and per-leaf rechecks use the same finite traversal.
Higher-ranked visit/visit_mut callbacks yield opaque short-lived leaf wrappers
with private config and reference loans. Typed getters expose only actual
SecretMatch or condition-HMAC group tokens; no public position/profile/source
constructor or arbitrary borrowed Condition/HmacMatchConfig binder exists.
Walk disabled entries to clear their resolved credential fields, then suppress
acquisition and reference status. Preserve existing placement validation and
entry/body seal, restriction allow/deny, RAP unavailable-candidate and dye miss
policies. Expanded/imported Secret refs bind to EP namespace/source/dependency
owner; the ConfigData origin retains only its own ConfigData authorization.

Refactor Controller condition credentials to prepare after expansion and consume
these loans. Standalone HMAC consumes existing PluginBindings secret groups in
supported request placement. Extract the existing decode/accumulation function
so both consumers share trimmed nonempty fields, first-wins group/reference
ordering, and retained-entry length warnings. MetadataPolicy is derived privately:
standalone uses actual upstream_header_fields, condition HMAC always ignores
metadata. SecretMatch reads its actual selected data key, rejects absent/invalid
UTF-8/whitespace-only values, retains successful value bytes unchanged, and clears
before publication. Its status path uses the same readonly owning visitor.

Verification includes loan compile-fail tests, root/leaf/group/profile reorder and
owner mutation rejection, disabled clearing, malformed/no-edge refs and exact
cross-namespace Grant transitions, actual handler imported RAP conditions with
EP locality, SecretMatch selected-key/whitespace preservation, and top-versus-
condition HMAC metadata plus first-wins regressions. Existing expansion/IP
resolution, no-I/O handler work, DAG, redaction and consumer failure policy remain
unchanged. Relevant files are Resources refs/owned + condition_fields + mod and
mirrored tests, Controller condition_tree, HMAC resolver, credential mod, EP
parse/status and mirrored tests, and owning condition/HMAC knowledge pages.

Scoped condition implementation review ref_async_impl_review_r1 (Astra xhigh):
PASS after correcting inactive wrong-stage restriction/RAP traversal. Fresh
Resources7 and two HRTB compile-fail tests pass. EP-handler102 pass, covering
existing imported RAP/grant/first-wins/metadata regressions and new default,
empty/core/malformed identities, no-edge invalid targets, exact SecretMatch
whitespace and inactive material clearing. The two new handler fixtures initially
encoded forged HMAC bytes as a JSON string; corrected to a byte array, then reran
the complete handler suite successfully. Condition/HMAC knowledge is updated.

### Paired local TLS material proof implementation slice

Independent concrete plan review ref_async_impl_review_r1 (Astra xhigh): PASS.
Scoped implementation review: PASS; required missing-nested-field, supported
Wasm response-stage and owner-drop coverage added before verification closure.

Readonly inventory by ref_schema_inventory recommends a serial Resources/Common
slice before global and transport migration. Add opaque `refs::outbound_tls::BoundOutboundTls`
constructed only by actual BoundPluginEntry/Mut and BoundLinkSysConfig/Mut
accessors. Recheck reference topology and placement/enable first. Private origins
retain the actual typed config borrow and owning prepared references; no public
raw policy/material/namespace/pair constructor exists. Match actual fields for
OIDC/LDAP external material, ExtProc, Wasm fetch, ForwardAuth/Guardrail/AiGuard
connections and every existing LinkSys provider including OAuth2. No Secret,
policy or PEM copies are introduced. Expose policy/material borrowing and ordered
paired CA visitation plus the paired client slot, retaining the whole field loan.

Common adds `validate_outbound_tls_field` and a privately constructed local proof
that retains this loan. Validate every actual token, exact CA cardinality and
author-order pairing, target name/namespace and exact kind provenance: ConfigMap
requires the reserved exact ConfigMap marker; Secret CA/client require marker
absence, including rejection of unknown/empty markers. Reject extra material with
no reference and missing material with a declared reference. Duplicate refs retain
distinct ordered slots. Do not parse PEM, inspect UID/RV, query stores/grants,
emit sensitive diagnostics or run proof on request/query paths.

Disabled/missing local TLS returns no active local loan, preserving separate
transport decisions and OTLP HTTPS global inheritance. LinkSys's unsupported CA
skip remains Controller no-query behavior; Common cannot claim proof after
filtering declared unsupported refs. An active unprovable declaration fails the
proof even when verify=false. This intermediate proof covers local fields only;
do not invoke raw global defaults and label the result fully prepared. Existing
raw entry points temporarily remain unmigrated work, without compatibility aliases.
Subsequent serial work must bind EGC true-cluster explicit namespaces and preserve
independent global CA/client axes, then introduce full preparation and gate all
fingerprint/RV/equality reuse sites against one global snapshot.

Frozen files: Resources refs/outbound_tls + mod + plugin_fields/link_sys_fields and
mirrored unit tests; Common link_sys_core/outbound_tls and mirrored unit tests;
task evidence. Verification covers every actual location, malformed identities,
missing/extra/reordered/duplicate materials, same-name Secret/ConfigMap and marker
failures, inactive/missing nested fields and compile-fail construction/owner-loan
boundaries. No CRD/wire changes or client/reuse behavior claims in this first slice.

### Owning outbound request-template and managed plugin TLS slice

Plan review: Astra PASS after clarifying lexical template identity and counting
only SecretRef ordinals. Scoped production review: Astra PASS; verification below
is partial. Fresh Controller tests: template credential 9, EP handler 104 and
LinkSys handler 34 passed. Resources refs 56 passed before the final test-only
non-vacuous stage fixture correction; two template borrow compile-fail tests and
the corrected fixture still require fresh verification. Reference contract
generator and unit-test layout (990 mounted files) passed. Controller now uses
one `managed_http` resolver for the three
finite actual plugin roots, removing their raw-name resolver modules. Webhook and
readonly plugin status consume actual template loans. Whole-template compiler
errors remain Accepted diagnostics, separate from malformed compiled SecretRef
DecodeError. No full-task completion or runtime/global TLS proof is claimed.

Prior local TLS slice fresh evidence: Resources refs 50 unit tests and 7 doctests
passed; Common outbound TLS 23 unit tests and 3 proof compile-fail doctests passed.
These results predate the subsequent template edits and are not final checks.

Readonly ref_owned_fields_impl_audit traced actual EP and LinkSys Webhook request
surfaces. Extend the single private OutboundRequestSpec template walker with a
Resources-private position enum; reuse it for admission, existing collection and
owning snapshots. Cover path/method, args/headers/cookies Template sources and
Json/Raw/Form body templates; Value/origin fields are not templates. Record real
position/body/source discriminants and ordered reference descriptors, never
retain or hash a template body or literal credential. Extract actual compiled
SecretRef segments with their segment ordinal, exact raw map key and selected
data key through the existing parse_secret_ref grammar.
Re-loans scan current actual templates through this same parser. Literal-only
changes do not invalidate reference authority; reference/key/order/container
changes do. This keeps async frozen revisions reference-only.

Actual owning roots are ForwardAuth.request, Guardrail.request,
AiGuard.evaluator.request and LinkSys SystemConfig::Webhook.request. Bind stage,
entry enablement, plugin/provider discriminant and evaluator presence. A private
template wire/contract chooses fixed core Secret identity, with explicit target
namespace; no raw names-plus-owner binder or public position constructor exists.
Named opaque request_secrets loans expose only these actual segments and tokens.

The existing grammar splits the last colon and first slash and requires nonempty
namespace/name/key. Preserve it, including names with additional slash/colon,
case and opaque data keys. A narrowly private fixed-template Secret identity
constructor checks these lexical components without DNS tightening; it accepts
no group/kind/profile/default. Actual resource owners/sources and ordinary
SecretObjectReference/CA/condition fields still use strict identity validation.
OwnedTarget records the private lexical provenance so both preparation and
borrowing choose the correct constructor. Malformed segments produce an invalid
token and deterministic template decode status, without a fabricated edge/read.
Template namespaces never default to EP/LinkSys namespace.

Controller request_secrets gains the owning-token loop and readonly status using
the same field/key/map-key loans and existing read_secret_key behavior (including
empty successful values and string_data fallback). Replace EP ForwardAuth,
Guardrail/AiGuard and LinkSys Webhook caller-collected names paths together; remove
the obsolete raw template helper API after migrating its finite callers/tests.
Use synchronous computation from cache; no handler IO or await is introduced.
Clear template and TLS material in all four EP stage arrays before activity gates.
Use existing bound TLS acquisition/status for these three actual connection roots,
including the approved ForwardAuth local TLS wiring. TLS-disabled blocks remain
independent of active templates. Add ForwardAuth explicit decode/denial status;
keep deterministic dedup and current shallow credential policies. Preserve
Guardrail/AiGuard POST/method admission and runtime readiness behavior.

Frozen files: Resources common/outbound_request + refs/identity/owned/plugin_fields/
link_sys_fields and any private template helper, mirrored tests; Controller
credential/request_secrets + mod, EP and LinkSys parse/status, FA/Guardrail/AiGuard
resolvers and shared credential status, mirrored tests; relevant original template
and plugin review knowledge. No public wire/CRD shape change. Service target
owner-namespace compilation remains the separately reviewed routing slice.

Verification: all template surfaces and excluded Value/origin fields; exact map
and data keys and preserved lexical examples; malformed/no-edge/no-read, source
owner/stage/evaluator/reference/key/variant drift, literal-only stability, valid missing/denied recovery,
readonly no-mutation, empty/string_data values, TLS-disabled template acquisition,
all-stage inactive clearing and actual ForwardAuth CA/client/Grant transitions.

### Next serial slice: EGC owning TLS and administrative selections

Astra independent plan review: PASS. This is the next serial implementation slice;
full global TLS/client consumption and final full-manifest reviews remain open.
Implementation now uses actual cluster bindings and dedicated typed administrative
cache acquisition. Resources refs 61 unit tests passed, including corrected template
stage fixtures; Controller lib check and 18 EGC tests passed. Resources 13 refs
compile-fail doctests passed, including two template loans and four EGC boundaries.
Astra scoped implementation review: PASS after correcting stale legacy CA helper
documentation. Full task verification is still open.

Workspace checkpoint: HEAD advanced externally from the original 73c1d7e81
baseline through c7c80585b/525ee0951 to 4dbcc5495. This workflow made no commits.
Revalidate the manifest and final diff against the current HEAD before closure.
The existing internal HTTPRouteExternalAuth::Edgion constructor also needs actual
route owner context in the later managed Service slice; its upstream ExternalAuth
branch remains unsupported and gains no aliases or protocol support.

The explicit approval in 00-context supersedes the historical EGC product gate.
Add Resources `refs/egc_tls_fields` with a reference-only PreparedEgcTlsReferences
and actual immutable/mutable owner bindings. Snapshot only real owner metadata,
finite outbound/validation/list/client presence and ordered CA/client wire slots;
never policy/PEM/Secret/full EGC copies. Actual EGC source is always Cluster;
missing/invalid owner name or any authored metadata namespace is invalid.
Targets require explicit valid nonempty namespaces. CA kind remains required and
supports Secret/ConfigMap; client defaults kind to Secret. Only the approved
custom group defaults and core spelling apply, never arbitrary group aliases.

Extend private owner/slot/topology for these exact EGC fields. No EGC asynchronous
freeze is needed: reject it rather than widening credential async protocols.
Expose opaque EgcTlsMaterialSelection only from actual CA/client field visitors,
retaining the owning token and private role. No raw constructor, arbitrary token
upgrade, source/namespace override or Grant-bypass boolean is permitted. Visit
all ordered CA slots, including invalid and repeated declarations. Add separate
BoundGlobalOutboundTls borrowing actual OutboundTlsConfig and its ordered pairs;
do not convert/copy it into local policy. Current material writes remain possible
after ending a loan, and fresh loans recheck owner/reference topology.

Controller `credential/egc_tls` accepts only these administrative selections.
Validate exact EGC Cluster source and exact dependency owner (kind/name/namespace
None), then register valid material recovery dependencies before store lookup.
Read cache only; do not query ReferenceGrant or add fictitious cross-namespace
Grant dependencies. Ordinary Secrets sanitize the reserved marker; ConfigMaps
use the canonical actual-object wrapper. No requested identity rewriting, new
Secret whitelist, PEM parsing, IO or fake authorization revision is introduced.
Resolve CA/client axes independently and traverse all valid slots before writeback.

EGC parse clears prior materials/dependencies first, binds its actual two fields,
then writes results after ending loans. Preserve plugin refs, GatewayClass requeue
and delete cleanup. Intrinsic identity validation runs through the same pure
visitor in preparse and existing Accepted status. Missing/unwatched objects,
material parsing and Grant presence must not alter deployment-independent shared
EGC Accepted or introduce shared ResolvedRefs. SAN/hostname Phase1 guards remain.
Always check cluster owner identity even when outboundTls is absent. Cleanup and
dependency ownership use canonical EGC/name/namespace=None, so an invalid authored
metadata namespace cannot preserve dependencies of the prior valid cluster owner.

Frozen files: Resources refs/{egc_tls_fields,owned,outbound_tls,mod} and mirrored
tests; Controller credential/{egc_tls,mod}, handlers/edgion_gateway_config and
mirrored tests; owning EGC architecture/config/auth documentation and current
spec/manifest rows. No wire/CRD/Common/Gateway/provider/replay changes in this slice.

Verification: exact cluster owner and target namespace matrix, custom group/kind
defaults, owner/ref drift, private constructors and overlapping mutable loans;
no-Grant selection and Grant-independent results with no phantom dependency;
ordinary namespaced consumers still denied without Grant; Secret/ConfigMap same
name/provenance, ordered duplicates/invalid/missing siblings, independent axes;
update/block/ref/delete cleanup and recovery; two runtimes with different material
visibility retain identical intrinsic Accepted and readonly identity validation
does not register dependencies. Update old fake-default-namespace Grant fixtures.
Global aggregate snapshot, Common preparation, all final client consumers and
reuse guards remain required subsequent work; this slice is not full TLS proof.

### Next serial slice: Common global source snapshot and publication

Reuse one private material-pair checker for local and global CA/client proofs.
Extract ordered CA/cardinality and client presence helpers through a private
closed local/global adapter; retain the sole name/namespace/kind-marker checker.
No public raw pairs, Secret slices, namespace, scope or profile proof constructor.
Global CA and client proofs remain independent so local overrides can mask exactly
their own inherited axes, without concealing a still-inherited failure.
Every EGC additionally contributes its actual owner proof to inherited verify,
including absent outbound blocks and zero-reference configurations. An explicit
local verify value masks only that third axis; CA and client proofs remain required
when inherited. Default System/no-client output fabricates no resource authority.

Common global candidate retains Arc<actual EdgionGatewayConfig> plus private
PreparedEgcTlsReferences; no detached policy, Secret or whole-resource copies.
Construct it only from actual typed EGC. Invalid source/field proofs remain in the
candidate and cannot filter original declarations out of conflict counting. Re-loan
the retained immutable owner when borrowing selected inputs. Opaque global snapshot
aggregates candidates, retains selected handles/proof failures, fixed redacted Debug
and bounded conflict flags; no serde/schema or independent resource registry.
Bootstrap default is verify=true/System/no-client without a fake EGC resource.

Preserve actual aggregation semantics: empty input true, every absent outbound
block contributes verify=true, nonempty all-false yields false. Validation candidates
have nonempty CA refs or wellKnown set; SAN/hostname-only and empty validation do
not count. Zero selects System; one retains the whole selected validation and
material proof; multiple preserve conflict/System and clear inherited validation/SAN.
Client ref presence counts without validity filtering or dedup; zero is absent,
one retains proof/material and multiple remain a declared fail-closed conflict.
Valid+invalid CA declarations still conflict. Local validation shadows the global
validation/SAN axis as a whole; local client ref shadows only the client axis.
Preserve verify/SAN enforcement and provider skip-verify restrictions.

Replace the existing global store with one Arc<opaque snapshot> publication under
the existing lock (getter clones only Arc and releases the lock). No second mutable
raw store or materializing raw getter. Setter always publishes current provenance
but returns effective changed state for existing rebuilds. Compare borrowed selected
policy/material plus proof/conflict state, preserving old PartialEq full metadata
semantics; fingerprint remains a different consumed-material identity excluding
UID/RV. Preserve the conflict client representative's borrowed tuple for equality,
never for material acquisition. Do not substitute fingerprint equality as an
unrequested rebuild optimization.

Gateway EGC map becomes HashMap<String,Arc<EGC>>. Full and partial publication
clone keys/handles only, insert original add/update handles, and aggregate from the
post-update map. Borrow for runtime/listener/DNS and other publishers; preserve
all existing unrelated min/max rules and publication scopes. Global snapshot retains
original selected owner handles and emits conflicts through existing Gateway logic.

Migrate direct global-getter consumers together: Common outbound_tls/tls/http_client
tls_defaults slot, dns/mod and dns/http_fetcher. Canonical resolution uses a private
borrowed effective policy/material view rather than composing copied raw configs.
Existing raw-local and explicit raw-default entry points remain temporary unfinished
consumers until their owning migration; they cannot serve new snapshot publication.
HttpDns identity no longer makes a fake inherited TlsConfig or clones Secrets;
extract the existing fingerprint encoder's private borrowed components. One captured
snapshot supplies identity and fetcher build; new preparation APIs accept that
snapshot rather than loading another generation internally. Query paths retain
prepared clients and gain no global lookup, TLS parse or audit.
HTTPDNS identity includes effective source/proof errors independently of equal
policy/material bytes; proof is the gate before reuse, never fingerprint equality.
Ready-client reuse is rejected on any inherited proof failure. An unchanged failed
generation with no fetcher may retain its matching proof-state identity and fallback,
avoiding repeated preparation on refresh ticks; repair must replace that failure.

Frozen files: Common link_sys_core/{outbound_tls and its private global module,
fingerprint,tls,http_client}, dns/{mod,http_fetcher}, Gateway config/edgion_gateway/
conf_handler_impl; mirrored proof/global/TLS/DNS/aggregation tests, Common TLS test
support and existing etcd/ExtProc/LDAP raw setter fixtures; owning fingerprint/EGC/
unification knowledge and task evidence. Use actual EGC test fixtures, never a
production raw-config-to-fake-owner adapter. Valid fixtures need real identity;
retain intentionally malformed inputs unchanged.
Include Gateway test-support accessors 378_function_item and 386_function_item
under tests/support/config/edgion_gateway/conf_handler_impl_test. Borrowed setter
equality preserves raw reference Option distinctions and full selected metadata.

Verification: shared local/global pairing matrix, all candidate/default/conflict
combinations including invalid candidates and SAN-only noncandidate; axis-specific
local overrides and independent failures; policy/ref/material/proof/equality changes,
including full metadata-only rebuild vs fingerprint equality; full/partial/delete
default restoration and original Arc retention; private construction boundaries;
capture A, publish B and ensure proof/identity/build still use A, then new capture B.
Include malformed owner/no-outbound and malformed owner/verify=false with zero refs,
plus explicit local verify alone versus explicit overrides of all three axes.
Bootstrap trusted no-ref bytes, Gateway backend/health owning proofs, all local
transports and all four replay gates remain separately reviewed required work.

Independent Astra plan review: PASS with the third verify-input proof axis and
the exact equality/test-support requirements above. This approves only this slice.

Implementation now uses one private local/global MaterialField adapter and sole
material-pair validator. GlobalOutboundTlsSnapshot retains original actual EGC Arcs
and prepared references; independent verify/CA/client failures survive publication.
It counts declarations before proof, preserves CA conflict/System and fail-closed
client conflict behavior, and never creates a detached raw global config store.
Setter always publishes the incoming Arc and uses borrowed complete-material/raw
reference equality for rebuild signaling. Gateway full/partial snapshots retain
the original input handles. The obsolete raw aggregate_verify entry was removed;
its existing test identity now verifies the owning snapshot aggregation.
Common resolution borrows global inputs, and HTTPDNS proof/identity/build share
one captured generation with no copied global Secrets or fake inherited TlsConfig.
Scoped Astra audit identified and corrected failed-HTTPDNS-state reuse: ready
clients reject inherited proof failures; unchanged failed generations retain fallback.
Fresh verification so far: Common/Gateway library check PASS; shared Common core
123 PASS (4 pre-existing real-Etcd ignored); initial Common DNS filter 69 PASS;
Gateway EGC full/partial/default/conflict and Arc-retention tests 37 PASS. Corrected
Common library 344 PASS (4 existing real-Etcd ignored), four compile-fail doctests
PASS, Gateway LDAP 26 PASS, ExtProc 123 PASS and unit layout 993 mounts PASS.
Independent Astra scoped code/knowledge review: PASS after the HTTPDNS correction;
the canonical test-fixture mount correction received a separate narrow PASS.
Raw local transport APIs and outer replay consumers remain required unfinished work.

### Next serial slice: current-HEAD GrpcJsonTranscoder descriptor reference

Extend the private owning plugin walker with a fixed ConfigMap descriptor slot.
Its private wire stores only name/key and branch presence; fixed core ConfigMap,
actual EdgionPlugins namespace (default only when owner namespace omitted), no
authored group/kind/namespace, no alias or cross-namespace extension. The key remains
an exact binaryData selector and part of topology/frozen revalidation; preserve its
existing nonempty/253-byte intrinsic validation rather than inventing key grammar.
Inline descriptor contents are never copied or hashed into reference topology.
Enabled supported request entries alone expose an opaque named descriptor loan;
unsupported stages and disabled entries expose no target. Reborrow checks actual
entry topology; no raw name/key/profile/source constructor is public.

Reuse the existing bound ConfigMap acquisition pipeline with exact actual owner,
valid dependency-before-store behavior. Its canonical same-namespace authorization
branch returns the fixed revision without reading Grant state; add no bypass.
Malformed owner/name or stale binding creates no edge or store read. Invalid key
shape is rejected before acquisition by only the existing nonempty/253-byte selector
checks, independently of full configuration compilation.
Read only binaryData[key], retain descriptor byte cap/compile diagnostics, no data
fallback or CA-material conversion. Clear descriptor material/error in all four
stage arrays before any bind/acquisition and also on ingest preparse. Inline and
inactive generations discard forged/previous resolved fields.

Move descriptor acquisition after the existing actual PreparedPluginReferences
construction. Add a pure readonly owning descriptor status walk: malformed identity
reports InvalidReference directly, missing ConfigMap reports its existing missing
object status, present object/missing key remains the existing Accepted compile
failure rather than changing material status semantics. Status creates no dependency
edge. Extract a shared bound ConfigMap status result beside the existing bound Secret
status result if needed; do not reuse CA decoding for descriptor status. Existing
generic dependency status scan remains unfinished until its separate full migration.

Frozen source files: Resources refs/{owned,plugin_fields} and their mirrored tests;
Controller credential/{pipeline,ca_refs} only for shared ConfigMap status mapping,
handlers/plugins/grpc_descriptor, handlers/edgion_plugins and mirrored handler tests;
owning knowledge/manifest/task evidence. Do not change transcoding conversion,
Gateway attachment semantics, native ExternalAuth, vendored CRDs or unrelated code.

Verify exact fixed target/source/default namespace, selected-key and branch drift,
material writeback stability, private constructor/mutable-loan compile boundaries;
malformed name/owner no read/no edge/direct status, missing/recovery/delete, exact
binaryData key and no data fallback, invalid key shape no acquisition, all-stage
disabled/unsupported clearing, readonly status with no edge, existing compile cap.
The changed ExtensionRef attachment callers stay in the required routing slice.

Independent Astra descriptor plan review: PASS. A private checked-selector
GrpcDescriptorReference exposes its token/key only after exact selector validation.
Reference(owner/name) errors yield readonly InvalidReference; InvalidSelector stays
the existing Accepted failure without acquisition. This changes no missing-key
status semantics, transcoding runtime or attachment contract.
Narrow Astra-approved refinement adds Resources grpc_transcoding/mod.rs to the
frozen source list only to share its unchanged crate-private key-shape predicate
between intrinsic validation and the sealed selector loan.

Descriptor implementation scoped Astra review: PASS. Resources refs 65 tests and
16 doctests passed (10 existing ignored examples); Controller plugin handler 107
tests passed after the equivalent shared key predicate extraction. Exact key and
branch drift invalidate the loan; material writeback does not. Fixed ConfigMap
acquisition and readonly status share the bound pipeline; all four stages clear
material, and selector/missing-key failures retain existing Accepted semantics.
Generic dependency status and route attachment migration remain open.
Fresh credential pipeline tests passed 35; unit layout, generated reference
contract and diff checks passed. Agent-doc validation currently fails on three
pre-existing current-HEAD links to removed grpc-json-transcoding task files
(AGENTS.md, its CLAUDE.md symlink, and the request-body runtime-contract page).
Revalidate and repair that independent documentation gate before whole-task
closure; this scoped descriptor result does not claim all project checks pass.

### Next serial slice: owning local TLS selection and effective preparation

The current active-field proof can visit zero reference slots without checking its
actual owner identity. Its optional return also cannot be the final transport API:
a caller could omit an existing active field and substitute inherited defaults.
Introduce an opaque owning local TLS selection constructed only by the existing
bound plugin-entry and LinkSys accessors. It retains their reference-only topology
and the actual active loan, or the actual inactive/absent selection. No public
constructor, raw Option, arbitrary policy/material or scope input can mint it.
Disabled fields still lend no material and do not enable transport; transport
scheme and provider capabilities remain decisions of their existing adapters.
In particular HTTPDNS absent/disabled TLS retains native system trust/no client,
without new global inheritance; enabled fields retain existing inherited axes.

Every selection validates actual source identity even when absent, disabled or
zero-reference. Existing active-field proof additionally checks source identity
before cardinality/pair checking. The owning accessor still rejects stale branch,
enable, reference and owner snapshots. Selection validation consumes the selection
and returns a private-constructor Common proof; callers cannot swap a policy or
material after validation. An effective resolver accepts only that proof, one
captured GlobalOutboundTlsSnapshot and the existing fixed provider skip-verify
capability. It borrows actual local inputs (or an empty local override for an
actual inactive/absent selection), enforces the inherited three proof axes and
reuses the canonical PEM/SAN/client resolver. No resource/material copies, I/O,
Grant queries, tracing, mutable registry or wire/CRD fields are introduced.

Frozen source: Resources refs/{outbound_tls,plugin_fields,link_sys_fields} and
their mirrored TLS tests; Common link_sys_core/outbound_tls and mirrored TLS
tests; existing owning shared TLS knowledge and task evidence. Runtime consumers
and replay gates are the next dependent slice and remain required; retain current
APIs only while this tracked migration is incomplete, without raw fallback in the
new proof API. No bootstrap, health/backend, routing or transport behavior edits.

Verification: malformed actual owner with no TLS, disabled TLS, zero-reference
active TLS and verify=false; default omitted owner namespace versus explicit bad
namespace; all plugin/provider active/inactive locations; stale enable/branch/ref
and owner snapshots; proof loan prevents mutation/drop and external construction;
inactive material ignored, active mismatched material denied; independent inherited
axis failures and overrides, capture A then publish B, provider skip-verify policy.

Independent Astra plan review: PASS for this foundation, preserving inert transport
gates. Inactive selections retain the accessor's actual-owner borrow; an internal
active-marker/actual-field mismatch fails with StaleBinding. Source proof precedes
all optional/pair branches, and the sole pair checker and captured resolver remain
canonical. Future owning transport factories must still reject disabled/unsupported
entries before building clients; selection alone does not authorize a transport.

Implementation scoped Astra review: PASS. The actual active/absent/inactive loan
checks source identity, shares the sole material pair checker and retains owner
borrows. Resources refs 66 tests, Common TLS 33 tests, Resources doctests 19
(10 existing ignored) and Common compile-fail doctests 6 passed. Test layout and
diff checks passed. Capture A uses the existing guard's verified empty snapshot;
an explicit assertion documents that fixture and the targeted capture test passed.
All runtime consumers/replay gates remain required subsequent work.

### Next serial slice: actual HTTPDNS preparation and both reuse consumers

Common HttpDnsCandidate is opaque and retains Arc<actual LinkSys>, its private
PreparedLinkSysReferences and one captured Arc<GlobalOutboundTlsSnapshot>. Its
constructor accepts only actual LinkSys (with an explicit captured-snapshot
variant); only another SystemConfig variant yields None. Malformed actual HTTPDNS
metadata, references, pairing or configuration stays an existing failed HTTPDNS
generation with its authored fallback. Do not turn invalid authority into a
missing reference or a System resolver. No fabricated name/namespace from lookup
keys, raw HTTPDNS policy constructor or self-referential borrowed proof is allowed.

The candidate validates owning local selection before comparison. Enabled local
TLS additionally checks all inherited axes through the captured snapshot. Disabled
or absent local TLS retains native system roots/no client and does not consume
global TLS. Pure validation/comparison emits no configuration audit. Private
identity uses the canonical config/material encoder plus fixed local/inherited
proof failure tags. A ready prior client cannot be reused under incoming failed
authority, regardless of equal resourceVersion, policy or PEM. An unchanged failed
candidate with no client may reuse its failed handle; repairing proof must recover.

Candidate preparation reborrows its immutable actual owner, validates again and
builds from the same captured global generation. Keep the canonical HTTP client
factory: add an owned-selection entry which rejects raw tls/tls_defaults or
preconfigured_tls overrides, delegates the same private policy/build core, and
applies only the actual proved active field. Inactive selection builds native TLS
defaults without global inheritance. Extract existing TLS audit into one private
helper reused by raw temporary and proved resolution, with identical cold WARN
semantics. Extract the existing native reqwest application into one private helper
fed by the canonical resolved output. Retain current unsupported SAN rejection,
CA replacement, skip-verify and client identity rules. No parallel resolver or
another reqwest builder implementation. New owned entry cannot accept raw policy
or material to substitute local TLS.

Build query-only HttpDnsResolverConfig explicitly from preset/url/response/fallback
with connection=None; do not clone a complete configuration then discard its
copied Secret fields. Prepared clients retain no second TLS material copy. Query
behavior stays unchanged: System bootstrap, dial/literal egress classification,
proxy disabling, redirect denial, timeouts, pool bounds and streaming response cap.
ReqwestHttpDnsFetcher::new changes to actual Arc<LinkSys> input and delegates the
same candidate/owning build path; existing transport tests retain their cases.
Remove all detached prepare/matches_config/with_defaults APIs and update callers.

Gateway HTTPDNS publication constructs one candidate from its existing actual
LinkSys value (one cold Arc resource copy replaces the current detached config
copy), checks candidate.matches before reuse and consumes it for preparation.
Forced rebuild still replaces matching failed/ready handles. Every current failure
publishes latest configured fallback; no credential_readiness early return or
ordinary persistent-provider last-good behavior is introduced. Preserve source
event serialization, stopping old entries, counting and cap exemption.

Controller fetch_httpdns_owner returns Arc of the actual GET LinkSys, including
metadata. ServiceDnsState prepares/compares that same candidate under its existing
key before storing the outcome. Missing/wrong provider clears state and remains
System; invalid existing HTTPDNS retains failed authored fallback. Preserve applied
EndpointSlice fingerprint, raw-GET missing-resolved-material limitation, kube-less
default behavior and leader lifecycle; no Secret GET or new acquisition/status IO.

Frozen files: Common dns/{mod,http_fetcher}, link_sys_core/{outbound_tls,tls,
http_client}; Gateway link_sys/runtime/store; Controller services/dns_resolver/
service; mirrored DNS/resolver-select/HTTP-client/TLS/Gateway-store/Controller-DNS
tests and existing Common tests/httpdns_fetcher.rs transport fixtures; owning
HTTPDNS/fingerprint/unification knowledge and task evidence. No reference-annotation,
other provider, routing, bootstrap or backend-health migration in this slice.

Verification: actual source/default namespace and wrong-provider boundary; same-RV
valid-to-invalid reference/source/material, repeated failure and repaired source;
disabled malformed refs inert, re-enable checks; independent inherited override
axes; capture A/publish B keeps candidate proof/identity/build on A; redacted
address identity versus private material identity; force/delete/type-change/full
omission; Controller missing material/configured fallback and applied-fingerprint
preservation; construction-only audit, zero query-time owner/global/TLS reads;
existing real transport cap/classification/redirect/reuse cases and compile boundaries.

Independent Astra plan review: PASS, including the shared private HTTP policy/build
core and full actual-owner fetcher construction. Pure comparisons perform no PEM
parsing/audit; source failure participates even for absent/disabled local TLS,
and inherited failures participate only for active HTTPDNS TLS. Both outer consumers
must cover same-RV invalid owner without local TLS, repeated failed reuse and repair.

Implementation now retains actual owner/captured snapshot in HttpDnsCandidate and
uses it in both outer comparison/build sites. Invalid existing HTTPDNS publishes
authored fallback; inactive TLS checks source but consumes no global trust.
Canonical owned HTTP factory delegates the same private build/native TLS/audit
cores and rejects raw tls, tls_defaults and preconfigured_tls substitutions.
Query configuration copies only non-connection fields. Existing tests retain their
identities and now construct actual metadata-bearing LinkSys fixtures; rotation
fixtures declare matching refs/material instead of undeclared CA bodies.
Fresh Common/Controller/Gateway library check PASS; Common library 349 PASS with
4 existing real-Etcd ignored; Controller DNS tests 11 PASS including same-RV bad
owner and active reference/material/raw-GET failure/reuse/repair matrices.
Remaining latest Common comparison/constructor tests, real transport tests,
Gateway store tests, documentation guards and scoped Astra final evidence pending.

Latest Common library tests passed 350 with 4 existing real-Etcd ignored; Common
compile-fail doctests passed 8, including private/detached HTTPDNS construction.
Real HTTPDNS transport tests passed 14 and Gateway runtime store tests passed 37.
Unit layout (993 mounts), reference-contract drift and diff guards passed. Original
three dangling grpc-json-transcoding task links still fail agent-doc validation.
An OS24 exec-service launch failure was recovered using the available persistent
Node runtime for the same local commands; no other processes were stopped.
Independent scoped Astra final review: PASS after resuming the source and original
knowledge audit. The reviewer confirmed both reuse gates check actual-owner proof
before reuse, share the captured snapshot for identity/build, preserve failed
fallback and inactive TLS behavior, and retain the existing transport test cases.
The reviewer independently checked Common 350/4 ignored, Gateway 37, Controller
11, real transport 14 and compile-fail doctests 8. This closes only the HTTPDNS
slice; other consumers and whole-task final reviews remain required.

### Next serial slice: actual Webhook TLS preparation and replay

WebhookCandidate retains one Arc of the actual LinkSys, its reference-only
PreparedLinkSysReferences, and one captured global snapshot. Only the actual
Webhook variant creates a candidate. Source identity, whole active local TLS
pairing and inherited axes are checked before either digest or same-RV reuse.
Absence/disabled local TLS checks the source but uses native TLS defaults without
inherited EGC trust or client identity. Failed candidates cannot reuse a ready
client. Ordinary failure preserves the existing same-type last-good client;
forced rebuild still retires that client before readiness/preparation, and kind
changes/full omission retain existing retirement semantics.

Construction rebinds the immutable actual owner and uses the same captured
snapshot as proof and fingerprint. ManagedOutboundClient gains an owning Webhook
entry accepting the bound actual LinkSys, checking its fixed provider variant,
then deriving its connection/request/template material and actual source namespace
internally. A single private construction core shares current target, DNS,
egress, request compilation, retry, health and cap behavior. The still-unmigrated
EP/Route raw entry delegates that core and remains explicitly unfinished; it
cannot serve as an alternate Webhook factory. The owning entry uses only the
canonical proved HTTP factory and never substitutes a raw TLS policy/material.
Add a read-only source accessor on the existing bound LinkSys rather than
fabricating metadata from runtime keys. Omitted owner namespace means default;
an explicitly invalid namespace fails even without TLS.

Derive Service target and active-health target namespace from the owning source
without cloning/mutating WebhookServiceConfig. Preserve explicit target namespace
and URL behavior. Complete custom Service identity/strategy loans and EP/Route
owner namespace migration remain the subsequent ManagedTarget slice; this scope
does not claim to close their existing raw-reference gaps.

Fingerprint preparation borrows actual config and material. Build a redacted
JSON connection projection, remove inactive TLS and fill only the effective
Service namespace in that projection; never clone a complete config/Secret and
strip it afterward. Hash template material and local TLS with canonical Common
encoders. Add one shared Common captured-global TLS encoder and pure active-local
inheritance check, used by both Webhook and HTTPDNS. The encoder/proof do no PEM
parsing, TLS audit or IO. Consumed inherited policy/material participates in the
same-RV secret fingerprint, so changed defaults cannot hide behind that shortcut.
Fingerprints remain private/redacted. No request-time owner/global lookup is added.

The shared inherited encoder applies each actual local override before encoding
global verify/CA/client policy, material and fixed proof outcomes. Native inactive
HTTP TLS consumes none; an explicit local axis masks changes and failures on that
inherited axis. HTTPDNS adopts this same encoder, so masked global changes no
longer cause unnecessary private-identity changes. This is an in-process reuse
identity, not a wire contract. Test masked-axis stability and independent unmasked
changes, including the same-RV shortcut.

Frozen files: Resources refs/link_sys_fields and mirrored tests; Common
link_sys_core/{outbound_tls,fingerprint}, dns/mod and mirrored tests; Gateway
providers/webhook/{client,apply,mod}, runtime/store, plugins/http/common/
{http_client,outbound/client}, their mirrored tests; owning Webhook/fingerprint/
unification knowledge and task evidence. No CRD, Controller handler, other provider,
plugin factory, routing or bootstrap migration belongs to this slice.

Verification preserves existing test identities and adds actual-owner construction,
wrong provider, missing/empty source identity, omitted namespace, inactive invalid
refs, active malformed/foreign refs, ordered missing/mismatched material,
independent inherited axes and capture A/build after B publication. Both equality
and same-RV paths reject bad incoming proof; ordinary last-good, forced absence,
repair, full omission and kind-change behavior remain covered. Guard shared globals
in tests. Run affected library checks, Common TLS/DNS tests, Gateway Webhook/outbound/
store tests, compile-fail boundaries and layout/reference/diff checks; final
independent Astra review includes original knowledge conclusions.

Independent Astra plan review: PASS, including the owning API and masked encoder.
Source and inherited proof precede both shortcuts; construction retains the same
snapshot, native inactive behavior and existing lifecycle. Verification must
contrast masked versus consumed changes and assert comparison emits no TLS audit.

The current source/knowledge review requested an actual capture-A/publish-invalid-B
construction regression; it now runs in an exact isolated child with unwind-safe
global restoration, preventing parallel Gateway global-state races. Original
Webhook architecture/TLS-loading and serialized template material paths were
corrected, and the SSRF page's incorrect cluster-scope claim was changed to actual
Namespaced ownership. A separate current failure-policy audit confirmed that
ordinary custom Webhook Grant-loss retention still permits new typed lookups/calls
through the prior publication (baseline and accepted spec section 5 items 13/15).
That explicit custom boundary is documented; it cannot justify retention for
standard Grant-required fields. No lifecycle policy change was inferred.

Affected library check passed. Latest Common run passed 352 with four existing
real-Etcd ignored; Resources refs passed 67 and Common compile-fail doctests passed
8. Webhook provider r1 test compilation found three new test field paths, corrected
before r2; r2 passed 42 and r3 passed 43 including the isolated actual publication
test. Independent final review found absent versus disabled TLS hashing differed;
the active material filter and equal-runtime/RV-change reuse regressions corrected
it. Latest provider r4 passed 45, runtime store passed 39 and shared outbound passed
97. Fresh independent Astra source/knowledge/result review returned scoped PASS
after independently checking the latest execution evidence and corrections.
Unit layout first rejected the added source test-only wrapper; moving it into the
existing mirrored test module preserved identities and layout passed 993 mounts.
Reference-contract drift and diff checks pass. The three missing historical gRPC
task links were minimally corrected in the two owning actual documents, after
independent Astra plan PASS, preserving every normative bound and marking missing
historical evidence explicitly. Agent-doc validation now passes 1447 files, 91
entries and 1050 source-path scopes. Independent corrected-result review is PASS.
This closes only the Webhook preparation slice; actual custom Service target
normalization, other transport consumers and whole-task final gates remain open.

### Next serial slice plan: Redis owning TLS and persistent candidate boundary

The persistent runner currently fingerprints a raw LinkSys before reuse, then
independently reconstructs TLS while building each connection attempt. Redis
mapping resolves global defaults twice. Introduce an associated ApplyOwner
Candidate: prepare once from the actual Arc<LinkSys>, obtain its fingerprint,
and pass the same immutable candidate through every retry. Redis gets an owning
candidate; Etcd/ES/Kafka/OTLP temporarily use their actual Arc as mechanical
adapters, explicitly retaining their unfinished TLS migration. Preserve forced
withdrawal, ordinary last-good retention, generation fences, backoff, publication,
health ownership, handover and deletion ordering.

Common owns a private-field PreparedRedisTls, retaining the actual Arc<LinkSys>,
PreparedLinkSysReferences and one captured Arc<GlobalOutboundTlsSnapshot>. Its
closed Redis constructor verifies the actual provider/source and the whole active
local reference/material selection before reuse. Transport TLS is derived only
from actual enabled local TLS or parsed active rediss endpoints. Inactive local
references remain inert, including on rediss; implicit secure transport inherits
global defaults through an empty local policy. Pure inherited-axis proof precedes
reuse even for implicit TLS. No PEM parsing, Secret copies or TLS audit occurs
during preparation/comparison. Local overrides mask only their inherited axes.

Redis fingerprint keeps existing config/auth encodings and disabled-local identity
distinctions, adding the canonical masked inherited digest to its material digest.
A Gateway RedisCandidate keeps that fingerprint and Common preparation together.
Construction borrows config/auth from the retained owner, resolves the proved
selection against its captured snapshot once, and feeds the result into one shared
rustls conversion core. Existing raw rustls consumers delegate to that same core
until their owning migrations. Preserve system-root fallback, private-CA replace,
mTLS, verify=false rejection, unsupported SAN/hostname rejection and Fred's dial
resolver/reconnect behavior. Fred retains the prepared connector across reconnect;
runner retries retain the same captured candidate. No new logging or request-path
work is introduced.

Redis production detached from_config/from_linksys constructors are replaced by
an actual Arc<LinkSys> entry. Fred pool mapping accepts the already selected
connector and never reads global TLS or raw material. Unit mapping fixtures use
actual named LinkSys owners. Existing real-Redis integration fixtures migrate to
a test-util-only actual-owner helper; no raw production escape hatch is retained.
No auth-reference authority expansion is claimed; that separate migration remains
open. Add compile-fail privacy tests and proof-before-same-RV/zero-live matrices,
source failure without TLS, dormant refs/rediss inheritance, ordered pairs,
masked/unmasked global identity, capture-A/publish-invalid-B construction/retry,
pure-comparison audit checks, and ordinary/forced invalid-generation recovery.

Exact scope: Common link_sys_core/{outbound_tls,tls,mod} and its new owning Redis
preparation module/tests; Gateway runtime/runner, Redis {apply,client,config_mapping},
mechanical candidate signatures in the other four provider apply modules and fake
runner owner, affected mirrored tests, five existing Redis integration test setup
setup fixtures (four detached constructor calls; DSL already owns its LinkSys),
and owning TLS/fingerprint/Redis/unification knowledge. No Controller,
CRD, Gateway API routing, egress mechanism or production TLS backend changes.
Run affected library/all-target checks, Common TLS/preparation tests and doctests,
Redis provider and persistent-runner/store tests, unit-layout/diff/doc guards.
Independent Astra plan approval is required before these code edits; fresh scoped
result review follows verification. Whole-task final review remains separate.

Independent Astra plan review returned PASS after actual caller/runner/TLS audit.
Implementation now retains the actual Redis owner/reference topology/global
snapshot, prepares before plan_apply and carries the candidate across init retry.
The sole rustls conversion core is shared; Fred mapping consumes its prepared
connector without global reads. Production detached constructors were removed;
four integration setup callers use an explicit test-only actual-owner fixture.
One OTLP CLI startup fixture and existing store/fake-owner test calls additionally
need mechanical Arc/candidate adaptation to the associated-type signature; no
provider semantics or production CLI behavior changes. First all-target check
identified these test call types and RedisApply visibility; corrections applied.
Fresh execution and result review remain pending.

Redis scoped completion: latest Common library passed 357 with four existing
real-Etcd ignored; Gateway LinkSys passed 306, including candidate proof before
same-RV/zero-live planning, ordinary versus forced recovery and one prepare across
failed-init retry. Common private-owner compile-fail doctests passed 9; Gateway
detached-constructor compile-fail passed 1. Latest affected all-target check r4,
format, diff, docs (1447/91/1050) and layout (995 mounts) pass. The test utility
mount was corrected to the canonical module-relative name and separate test /
feature guards. Fresh independent Astra result review returned scoped PASS after
independently reading current logs and the corrected source/knowledge. This is
not live Redis TLS handshake evidence or whole-task closure.

### Next serial slice plan: shared actual LinkSys TLS loan and Elasticsearch

The next fixed-variant adapter is Elasticsearch. Its incoming Arc currently has
no owning preparation and its fingerprint omits inherited TLS. Its sole production
construction chain copies enabled TLS into the raw HTTP factory. Bulk and recovery
already reuse the constructed client and must keep doing so; init intentionally
publishes unhealthy after a failed initial health probe and does not become a
retrying connection handshake.

Extract the common actual LinkSys container into private-field PreparedLinkSysTls:
actual Arc<LinkSys>, PreparedLinkSysReferences, one captured global snapshot.
Construction proves actual source and whole active local pairing through the
existing canonical checker even with absent/disabled TLS. It accepts no caller
provider selector, raw TLS, refs, materials or source metadata. Its immutable
binding/callback exposes only actual loans and captured defaults, never mutable
owner access. PreparedRedisTls delegates source/pair/snapshot orchestration to
this container while retaining its substantive closed Redis/scheme/implicit
inheritance/hash/rustls behavior. This is reuse of a shared binding, not a legacy
alias or another identity/material implementation.

Private EsCandidate requires the actual Elasticsearch variant, checks active-local
inherited authority purely, and computes one fingerprint including existing
configuration/auth/local encodings and canonical masked inherited axes. Native
absent/disabled HTTP TLS consumes no globals even for HTTPS URL targets. Enabled
TLS with empty local policy inherits defaults. Invalid actual source or active
pairing fails before runner digest/RV/zero-live decisions. The same candidate
supplies construction, retry and generation fingerprint.

Replace EsLinkClient production detached constructors with actual Arc entry and
private candidate construction. Its private configuration builder obtains config
and resolved_state from the retained actual owner. Preserve Basic/API-key/Bearer
header construction, sensitive Authorization flags and invalid/pending rejection.
Keep header assembly as a pure shared helper for its material-shape unit tests;
production passes the resulting headers into the canonical factory by value.

Add a fixed-header owning build entry to Common's HTTP factory. The factory creates
its own builder and delegates the existing private policy/conversion core. It
accepts only typed headers as extra customization, not a preconfigured builder.
Reject raw tls/tls_defaults/preconfigured_tls in its config exactly as existing
owning construction does. ES no longer clones TLS/Secret material or returns an
unused copy of auth headers. Keep Redirect::Limit(10), proxy enabled, system DNS
non-adoption, timeout/pool clamps, endpoint ordering/rotation, bulk settings and
health/lifecycle semantics. This TLS slice does not authorize egress changes.
Preserve supported/audited verify=false, custom CA replace, mTLS and unsupported
hostname/SAN rejection through the canonical conversion core.

Exact scope: Common outbound_tls.rs and new outbound_tls/link_sys.rs, the Redis
adapter as a supporting consumer, http_client.rs, their mirrored tests; Gateway
Elasticsearch {apply,client,config_mapping} and mirrored tests, mechanical existing
store test candidate calls; original shared TLS/fingerprint/unification knowledge
and this evidence record. No Controller, Resources/CRD, other provider transport,
production runner/store, auth-reference authority or backend TLS migration.

Verification: shared immutable-loan compile-fail boundary; wrong actual source
without TLS, whole ordered CA/client identity/cardinality/provenance, malformed
target and undeclared material; disabled inert refs and HTTPS native behavior;
masked versus consumed global changes and same-RV identity; actual capture-A /
publish-invalid-B construction; preparation/compare emits no TLS audit and
construction retains it; owning fixed-header factory rejects all raw overrides;
detached production constructor compile-fail; existing config/header/pool/bulk
and recovery tests, Redis supporting tests and runner/store lifecycle regressions.
New HTTP servers belong to integration targets, not these unit tests; no live ES
handshake claim follows pure/fake tests. Run affected all-target checks, Common
tests/doctests, Gateway ES/runner/store tests, format/diff/layout/doc guards.
Independent Astra plan approval is required before code edits, followed by fresh
result review. Other provider/auth migrations and whole-task closure remain open.

Independent Astra plan review returned PASS after original source/knowledge audit.
Implementation now shares actual owner/reference/default orchestration between
Redis and ES, retains fixed provider gates, and creates fixed-header clients only
inside the canonical factory's sole private build/policy core. Production ES
detached constructors and unused returned auth-header copies were removed; pure
header fixtures preserve existing test identities and sensitivity checks.
The unification page's old 'CLI is the only proxy-enabled consumer' claim was
corrected to describe current ES behavior explicitly without approving a new
general exemption. First all-target check passes; latest Common library passes
361 with four existing real-Etcd ignored. Gateway tests and fresh result review
remain pending. Whole-task documentation reconciliation must additionally fix
resources/18-link-sys.md's stale pre-migration Webhook template/fetch description.

ES fresh verification: Common library passed 361 with four existing real-Etcd
ignored; Gateway LinkSys passed 310; Common compile-fail doctests passed 11 and
Gateway detached ES constructor compile-fail passed 1. Latest affected all-target
check r2, format, diff, docs (1447/91/1050) and layout (996 mounts) pass.
Fresh independent Astra result review returned scoped PASS after independently
reading corrected source, knowledge and current logs. No live ES handshake or
whole-task completion is inferred from these checks.

### Next serial slice plan: actual owning Kafka TLS and retained reconnect input

Reuse PreparedLinkSysTls directly in a private KafkaCandidate, fixed to the actual
Kafka variant. Validate source and whole active ordered local pairing, then
active inherited authority before computing the cached fingerprint. Include
canonical consumed inherited TLS axes in the existing local/auth encoding. These
preparation/comparison steps remain pure and precede runner digest/RV/zero-live
decisions; the existing runner retains the candidate across initialization retry.

Add one fixed owning Common Kafka rustls entry accepting only PreparedLinkSysTls.
It borrows the actual selection and captured defaults and fixes SDK capability
to provider_can_disable=false. Absent/disabled local TLS yields None; active TLS
must yield a concrete connector, including System roots without mTLS. Installed
rskafka 0.6 transport uses None as plaintext and supplies no default TLS builder.
Split the existing sole Common rustls core into concrete conversion and the
existing optional-native-default wrapper. Preserve raw/Fred System-without-
identity None behavior, custom roots replacement, mTLS, unsupported verify=false
and hostname/SAN rejection. Remove Kafka's duplicate root-store algorithm.

Private runtime connection input retains Arc<PreparedLinkSysTls> and the connector
Arc built once during cold runtime construction. Brokers and SASL configuration
come from the actual owner; resolve authentication once into that input and copy
only strings required by the SDK at connection construction. Init and probe
threshold recovery clone this input Arc and reuse its connector; no raw config,
TLS Secret clone, PEM parsing or global-default reread occurs during reconnect.
Separate runner initialization-attempt clients may construct their connector
again from the same immutable captured candidate; no lazy cache is needed.

Replace detached production from_config/from_linksys with an actual Arc<LinkSys>
entry and private from_candidate. Keep queue limits, health, backoff, retirement,
forced versus ordinary publication, broker DNS and SDK reconnect behavior. Auth-
reference authority and egress remain separate outstanding slices. Migrate
existing unit fixtures through actual named owners, preserving test identities.

Frozen allow-list: Common src/link_sys_core/tls.rs and mirrored tls_test.rs;
Gateway Kafka {apply,client}.rs and mirrored apply_test.rs, client_test.rs and
client_test/tls_config_tests.rs and two mechanical candidate callers in mirrored
runtime/store_test.rs; original unification, shared TLS loading/shared
fingerprint and kafka-aggregate-queue-bounds knowledge; this evidence record.
No Resources, Controller, CRD, Cargo, common container, production runner/store,
Redis implementation or Gateway backend-TLS edits.

Verify actual source without TLS, disabled inert refs, wrong provider, active
pair identity/cardinality/provenance, pre-reuse failures, masked/consumed inherited
identity, isolated capture-A/publish-invalid-B repeated construction, explicit
System TLS Some versus inactive None, custom CA/mTLS and capability rejection,
retained owner/connector Arc and pure preparation audit, detached constructor
compile-fail, existing Kafka queue/lifecycle and runner/store regressions. Run
focused Common TLS/Redis support and Kafka/runner/store tests, doctests, affected
all-target checks, format/diff/layout/docs guards. Pure connector tests are not
live Kafka handshake evidence. Independent Astra plan review returned PASS after
original source, rules, knowledge and installed SDK audit. The Common entry itself
must check the actual Kafka variant before inactive TLS; constructor-derived SASL
Pending/Invalid must fail fast without anonymous degradation. Fresh scoped result
review and whole-task final reviews remain gates.

Supporting scope addition independently approved by Astra: the removed Kafka
detached constructor was the last production unspecified-fingerprint consumer.
Move RuntimeFingerprint::unspecified into an impl in existing mirrored
runtime/apply_test.rs and fix its stale comment; its remaining three calls are
existing test-only fixtures. The initial source cfg(test) gate was rejected by
the unit-layout guard; this exact corrective move received independent Astra
plan PASS without adding guard exceptions. Production is_unspecified, visibility
and replay planning remain unchanged.

Kafka implementation now retains the actual owner, captured snapshot and cached
masked inherited fingerprint before replay. The fixed Common converter builds
enabled System TLS explicitly through the sole rustls core while preserving
Fred native-default fallback. Runtime connection input retains the prepared owner
and connector Arc through init/probe recovery without raw config/Secret copies
or global rereads. Detached production constructors are removed; SASL readiness
fails fast at actual owning construction. Mirrored fixtures preserve old test
identities; new source/pair/pre-reuse, capture/publication/retention, capability,
pure preparation and detached boundary tests are under fresh verification.

Kafka scoped completion: Common library passed 364 with four existing real-Etcd
ignored; full Gateway LinkSys passed 315 after the mirrored helper correction;
latest corrected Kafka provider tests passed 26. Common compile-fail doctests
passed 11; Kafka positive actual-owner and negative detached-constructor doctests
passed 2 with the real import path. Latest affected all-target check r3, format,
diff, docs (1447/91/1050) and layout (996 mounts) pass. Independent Astra result
review returned scoped PASS after independently reading corrected source, SDK,
original knowledge and current logs. The new inherited capability fixture initially
failed because it omitted System activation of the global validation axis;
corrected inherited-only fixtures and reran successfully. Pure connector tests
are not live Kafka TLS handshake evidence or auth-reference authority completion.

### Next serial slice plan: owning Etcd and explicit trusted bootstrap

Gateway Etcd uses a private fixed-variant EtcdCandidate over the existing
PreparedLinkSysTls Arc. Source, whole active ordered local pairing and consumed
inherited authority precede fingerprints, same-RV and zero-live planning. Keep
existing normalized endpoint/config/auth/local TLS encodings and add the canonical
masked inherited encoder in the secret/material fingerprint component (the RV
shortcut ignores rebuild inequality). Absent/disabled TLS consumes no Edgion
defaults even for HTTPS endpoints; this preserves SDK-default TLS handling,
without asserting system-root trust. Enabled local TLS consumes the captured
generation.

Common EtcdLinkClient construction accepts either the actual prepared LinkSys
or an explicit EtcdBootstrapConfig with scalar connection fields, direct optional
BasicAuthMaterial and optional existing non-Clone/redacted TlsMaterial. Bootstrap
accepts no resource, TlsConfig, Secret, reference token or invented metadata.
Only Controller calls its production entry. Gateway calls only the actual owner
entry. Remove public detached from_config and raw reference mapping constructors.
Actual auth is derived from its retained owner and remains fail-fast when unusable;
this does not claim completion of auth-reference material authority.

Reuse the existing canonical TLS core through private borrowed input adapters.
LocalInputs contains optional verify/whole validation selection, a client
declaration flag and borrowed material; ValidationInput contains borrowed policy
and CaInput (System, expected-count Secrets, or direct PEM); ClientInput borrows a
Secret or direct cert/key pair. Existing resource adapters mechanically project
their actual policy/material. Bootstrap fixes verify=None, local validation only
when disk CA exists, client_declared=false and cert/key zip (partial pair ignored).
One resolve_effective_tls performs precedence, unsupported CA mode, SAN forcing,
declared-client presence and provider capability checks, then materializes the
existing ResolvedOutboundTls. Do not synthesize Secret, reference or local config
to express disk CA. Keep source/material proof at the actual resource entry and
real inherited EGC proofs at both captured-snapshot entries.

GlobalOutboundTlsSnapshot's existing effective_proofs delegates to one private
effective_proofs_for mask helper; resource and bootstrap adapters derive masks
from the same private input selection. No public boolean proof bypass. Add a
pub(crate) fixed Etcd bootstrap audited resolver taking only TlsMaterial and the
captured snapshot (capability=false, provider=Etcd). Both Etcd entries use one
ResolvedOutboundTls-to-TlsOptions converter. Preserve custom CA replacement,
mTLS, verify=false and hostname/SAN rejection. SDK options/Identity Debug must
never be printed (SDK Identity Debug includes the private key).

Build ordinary and election options once. Construct a request-timeout-free base;
election gets its existing fallback dial timeout when base is otherwise empty.
Ordinary adds request timeout to the base, using a fresh empty options object
when request is the only configured setting (no unintended default dial timeout).
Without any setting ordinary retains its existing dial-only fallback. Private
runtime input retains actual owning preparation where applicable, the two options,
endpoints and probe timeout. Init/reconnect/per-endpoint probe/election clone SDK
options from this input; they never rerun Edgion TLS resolution or globals.
Tonic/etcd-client can still clone or parse retained PEM internally at dial; this
is not a connector-Arc or SDK-secret-zeroization claim. ops::ping uses the saved
probe timeout; remove redundant full config/auth reconnect storage.

Controller conversion reads the same UTF-8 files and moves key bytes into existing
Zeroizing ownership, then calls the explicit bootstrap entry. Preserve absence
of TLS versus present empty TLS, disk CA masking the global validation block,
inherited global verify, global declared client priority over disk client pair,
failure without falling back to the disk pair, and ignored partial cert/key.
Preserve file errors and existing Controller-only 5s dial/5s-3s keepalive/idle
defaults, operator override and missing keepalive-half completion. Gateway gains
none of these defaults. No bootstrap byte cap is added or heap-bound claimed.

Reuse resource connection validation without fabricating an EtcdClientConfig:
extract the existing endpoints/timeout/keepalive checks into one pure borrowed
validate_etcd_connection_fields function in resources/link_sys/etcd.rs. Existing
validation_errors delegates unchanged; Common bootstrap uses that same function.
This supporting Resources edit changes no field, schema, default or admission
semantics and is needed to keep bootstrap and actual-owner validation identical.

Frozen scope: Common outbound_tls.rs and outbound_tls/global.rs; Etcd
{client,config_mapping,ops,mod}.rs; corresponding existing mirrored outbound TLS,
global and Etcd {client,config_mapping,ops,election} tests. Gateway Etcd apply.rs,
its mirrored tests and seven mechanical runtime/store_test.rs candidate callers.
Controller config/conf_center/etcd.rs and conf_mgr/conf_center/etcd/center.rs,
plus the re-export config.rs's stale conversion-name comment only;
mirrored config Etcd tests and storage/watcher/leader_election construction
fixtures. Resources resources/link_sys/etcd.rs and existing mirrored tests only
for shared connection validation. Original unification, shared TLS loading/FP,
etcd-configuration-and-message-boundaries and conf-center-etcd-abstraction
knowledge; this record. Existing run_etcd_test.sh section 11.4 and
04_LinkSys_default_etcd-tls-reject.yaml text changes only where rejection moves
from init to construction; preserve no-connect/ping/error assertions. No Cargo,
CRD, workqueue/handler, production runner/store, lease/CAS or egress edits.

Keep existing test identities through actual named/provenanced owning fixtures
or explicit bootstrap fixtures; private pure mapping tests may borrow config
fields, but test helpers establish no resource authority. Verify source without
TLS/wrong provider, ordered pairs/provenance/undeclared material/disabled inert,
pre-reuse rejection and ordinary/forced recovery, masked inherited identity,
isolated capture-A/publish-invalid-B repeated owning construction, saved options
and timeout differences/request-only fallback/clamps/keepalive/readiness, bootstrap
precedence/absence/empty/partial/file errors/redaction, pure comparison audit and
positive-owning/negative-detached compilation. Run Resources connection tests,
Common whole library/TLS doctests, Controller conversion/config-center tests,
Gateway Etcd/runner/store tests and affected all-target checks plus guards. Existing
real-Etcd ignored fixtures stay ignored; no live TLS/election claim from unit tests.
Fresh independent Astra plan PASS is required before these code edits; fresh
result review and whole-task multi-round review remain separate gates.

Independent Astra Etcd plan review returned scoped PASS after original rules,
knowledge, source/caller/test and installed SDK audit. The inherited digest must
enter secret/material identity; inactive HTTPS preserves SDK-default handling,
not a system-root guarantee. Existing keyed skip-verify error text is retained.

### Etcd slice implementation and pending final review

The approved Etcd slice now uses actual-owner candidates and immutable retained
SDK connection options, plus the explicit trusted Controller bootstrap input.
Disk PEM and resource material enter one private borrowed TLS precedence core;
there are no bootstrap synthetic Secrets/references. Existing connection-field
validation is shared by borrow in Resources. The Controller callers and six
Gateway store fixture construction calls have been updated. Current scoped
verification: affected all-target compilation passed in
`/tmp/edgion-ref-etcd-check-r3.log`; Gateway LinkSys tests passed 318/318 in
`/tmp/edgion-ref-etcd-linksys-r1.log`; agent-doc validation passed in
`/tmp/edgion-ref-etcd-docguard-r1.log`. Common and Controller tests and Common
positive/negative construction doctests are running; no final slice acceptance
is claimed. New hostname-inheritance fixture initially omitted explicit System
selection and was corrected to exercise the actual global validation axis.
Fresh independent Astra result reviewer `/root/etcd_astra_result_review` is
reviewing the actual diff and original knowledge. The whole task remains open.

### Approved next OTLP plan (read-only review complete; implementation pending)

Independent Astra reviewer `/root/otlp_astra_plan_review` returned scoped PASS
with these explicit constraints; Etcd's serial verification and final review gate
must complete before starting OTLP edits.

Common adds a closed `PreparedOtlpTls` wrapper retaining actual `Arc<LinkSys>`,
fixed OTLP variant, reference topology and captured global snapshot. It retains
full existing `LinkSys::validate_config()` behavior, including OTLP's explicit
valid metadata namespace/name requirement, before any reuse. Source proof always
runs; ordered local TLS pairing runs only for active local TLS, and disabled
references remain inert. HTTPS absent/disabled TLS consumes inherited axes via
empty local policy; HTTP consumes no inherited TLS and rejects enabled local TLS.
The canonical inherited encoder writes the secret/material digest, because
same-RV reuse ignores the rebuild digest. One private candidate supplies proof,
cached fingerprint and captured defaults to cold construction. Common uses the
existing canonical audited TLS resolver with capability false and no new SDK
or TLS parser dependency. Gateway retains the single tonic014 conversion,
explicit System webpki roots/custom replacement/mTLS/SNI/timeout/H2/egress policy,
and one retained cold transport input across retries. Existing bearer parsing
and auth metadata semantics remain; auth Secret authority is not claimed.

Frozen allowlist: Common `link_sys_core/outbound_tls.rs`, new
`outbound_tls/otlp.rs`, mirrored `outbound_tls_test.rs` and
`outbound_tls/otlp_test.rs`; Gateway `link_sys/providers/otlp/{apply,client,transport}.rs`
and their mirrored unit tests; two typed-candidate store test callers and one
CLI startup helper; `edgion-gateway/tests/otlp_transport.rs`; original unification,
shared TLS loading/fingerprint and OTLP ownership/egress knowledge; this record.
No Cargo, Resources, Controller or OTLP lifecycle changes are authorized by this
scoped plan.

Verification must cover invalid/missing/empty source metadata across HTTP and
HTTPS absent/disabled TLS; wrong provider/key; whole active CA/client pairing and
inactive references; same-RV inherited changes and correctly masked changes and
proof failures; proof before equality/RV/zero-live; captured valid A then published
invalid B with repeated construction from A and rejection of new B candidates
(global publication regression isolated in a child process); actual transport
CA/client name/namespace fixtures, declared client ref only when identity exists
so absent mTLS still reaches the live handshake failure; Common TLS/wrapper
unit tests and doctests, Gateway OTLP/runner/store/startup tests, and
`cargo test -p edgion-gateway --features test-util --test otlp_transport`, affected
compilation and required guards. A fresh independent result review remains
mandatory after implementation.

Etcd final-review correction: the fresh Astra reviewer approved a narrow private
scalar extraction to make ordinary/election timeout behavior executable without
SDK Debug, unsafe field inspection or network unit tests. `ConnectionTimeoutOverrides`
is temporary cold-path policy, not retained state or a public API. Exact tests
cover base absent/present crossed with request absent/present, request-only
ordinary no-dial vs election fallback, and request/dial clamps. One SDK application
function maps the policy, and empty bases use `ConnectOptions::new` to preserve
SDK constructor behavior. The original configured auth/TLS/dial/keepalive mapper
is unchanged. Pre-correction fresh verification passed Common 367/367 (four
existing ignored), Gateway LinkSys 318/318, Controller Etcd 37/37 (six existing
ignored), and Common doctests 13/13. Correction checks and Resources scalar suite
are running; final reviewer approval remains pending.

Etcd scoped acceptance: fresh independent Astra result review returned PASS and
reconfirmed PASS after the narrowly explained SDK lint annotation. Final evidence:
Common 368 passed/four existing ignored (`etcd-common-r4`), Gateway LinkSys 318
passed (`etcd-linksys-r2`), Resources Etcd 16 passed (`etcd-resources-r1`), Controller
Etcd 37 passed/six existing ignored (`etcd-controller-r1`), Common doctests 13
passed (`etcd-common-docs-r2`), affected all-target check (`etcd-check-r4`), format
(`etcd-fmt-r3`), agent-doc (`etcd-docguard-r2`), unit layout 996 (`etcd-layout-r2`)
and diff checks passed. Log prefix is `/tmp/edgion-ref-`. Affected-crate Clippy
exited zero (`etcd-clippy-r1`); Common rerun exited zero with no Common warnings
(`etcd-clippy-r2`). Existing whole-task Resources and Controller test lint
warnings remain for final cleanup; this is not a warning-free workspace claim.
`ConnectOptions::new` differs from derived Default, so its semantics are preserved
with a local documented lint exception. No test identities were removed across
the scoped mirrored files. No live Etcd TLS/election claim is made. The full
reference task remains open; next mutation is the independently approved OTLP
slice above.

OTLP implementation is now in progress under the approved allowlist. Common's
closed PreparedOtlpTls preserves explicit OTLP source validation and distinct
HTTPS implicit-global behavior; the private candidate proves before replay and
hashes inherited axes in the secret/material digest. Actual-owner constructors
replace detached entries; native transport retains that owning input and its
one cold Endpoint/metadata. Mechanical store/startup callers and live TLS
fixtures now use actual source/target identities. Common fresh unit tests pass
372/372, with four existing ignored; Gateway unit, feature-enabled check and real
transport tests are running. The new isolated real HTTPS case proves delivery
with the captured global CA after a later invalid EGC publication; no success
claim is made until that test runs. Independent final review and remaining
whole-task gates are still required.

OTLP scoped acceptance: fresh independent Astra result reviewer
`/root/otlp_astra_result_review` returned PASS after reading actual source,
original rules/knowledge and installed tonic SDK source. Latest fresh gates:
Common 373 passed/four existing ignored (`otlp-common-r2`), Gateway LinkSys 321
passed (`otlp-linksys-r2`), startup drain one passed (`otlp-startup-r1`), real
transport 10 passed (`otlp-transport-r1`, all nine existing identities retained),
Common doctests 14 passed (`otlp-common-docs-r1`), Gateway doctests six passed/seven
existing ignored (`otlp-gateway-docs-r1`), feature-enabled all-target check
(`otlp-check-r4`), feature-enabled Clippy (`otlp-clippy-r1`), format, doc guard,
unit layout 997 and diff checks passed. Prefix: `/tmp/edgion-ref-`. Clippy reports
three existing whole-task Resources warnings, with no Common/Gateway warnings;
final whole-task cleanup remains open. The real captured-CA test proves HTTPS
record delivery/SNI after invalid global publication. It does not prove auth
Secret authority or whole Gateway API conformance. No scoped test identities
were removed. The next serial mutation is the independently approved native
Route backend/Grant slice; all other reference consumers and whole-task reviews
remain open.

### Approved native Route backend/Grant slice

Independent Astra plan reviewer `/root/native_route_astra_plan_review` returned
scoped PASS. Resources adds static actual HTTP/GRPC/TCP/TLS/UDP owner visitors,
private position/source/terminal loans and distinct effective target loans.
The existing targets classifier remains the sole exact group/kind/name/namespace/
port implementation: standard omitted group is empty and omitted kind is Service;
explicit core and unsupported/pseudo kinds are rejected. Only authored valid
terminal loans mint GrantQuery. Named delegation and namespace wildcard selectors
stay separate, and effective loans/owned dependency receipts never mint Grant.
No serialized provenance field or whole-resource Arc copy is added.

Controller adds a private shared backend helper with terminal-only authority and
lossless cold dependency receipts (declaring source/position/port/target/key).
Root wakeup ownership, actual authorizing source and manager key representation
stay distinct: metadata-derived resource keys preserve omitted namespace, while
existing Service registration normalizes it to default. Cross-namespace dependency
registration precedes both the validation-switch read and Grant lookup; L4 first
installs its full valid target namespace set. Valid missing/denied terminals
retain wakeup edges before filtering; invalid targets produce no Grant/backend
read or fabricated dependency. Whole apply replacement and dropped-target policy
requeues remain intact; HTTP restores role-qualified mirror edges after clearing.

Five handler parse/dependency/status paths move from lossy raw tuples to actual
loans/receipts. HTTP AI uses exact targets and preserves slot/weight/fact-vector
shape, with invalid slots Missing and no erroneous Service/AI lookup. Delegation
borrows original child/rule/backend before cloning; child source authorizes, root
only receives wakeup dependencies, and clones retain explicit child namespace/
denial. Preserve named/wildcard recovery, child visibility, reverse consent,
cycle/depth, scope/filter rules and coalesced recreation. HTTP's old raw Grant
callback remains mirror-only until the later mirror slice; GRPC removes its native
raw callback. Parent/allowedRoutes/filter/mirror/policy/runtime selected-target
work is excluded from this slice. Immutable loans end before narrow denial
updates; no adapter exposes whole mutable backend descriptors.

Frozen production allowlist: Resources `resources/refs/{mod,targets,route_fields}.rs`
(the last new); Controller `handlers/{mod,route_backend_refs,route_utils,http_route,
grpc_route,tcp_route,tls_route,udp_route,route_delegation,grpc_route_delegation}.rs`
(new route_backend_refs). No Grant store/matcher/validator/events, Common, Gateway,
CRD, Cargo or workqueue changes. Mirrored tests: Resources refs route_fields/new
and targets; Controller helper/new, route_utils, http_route and its coalesced
recreate case, grpc_route, both delegation modules, tls_route, TCP coalesced
backend-TLS case and UDP multi-rule cases. Knowledge: task 04/06, original HTTP/
GRPC resource architecture, dependency-before-authorization, shared-Service
predicate and weighted-invalid-slot guards. A later authorized documentation
pass must correct common/TCP/TLS/UDP feature tables that call Service ports
optional without their conditional requirement; no schema default is changed.

Verification covers all five actual sources and defaults/invalid source metadata;
strict target/default/port cases and zero-read/zero-edge spies; private constructors,
borrow exclusivity and effective inability to mint Grant; Grant on/off and
register-before-read ordering; child-to-root namespace requiring child permission;
missing/denied recovery, named/wildcard/reverse consent and stale-denial reset;
duplicate target slots/ports without manager key proliferation; Service/AI
retargeting/coalesced recreate/dropped-policy requeues/mirror preservation;
status ownership, invalid-slot traffic weights and AI fact alignment. Run
Resources refs/doctests and focused then resource_processor Controller tests,
affected compilation and guards; existing Gateway weight/denial regression tests
are read-only verification. Independent fresh final review and later live
admission/conformance remain mandatory.

### Checkpoint corrections before the next serial implementation batch

Independent checkpoint plan review approved a private borrowed gRPC context guard
and its exact-identity matrix, one justified immutable-client selection invariant,
and the introduced lint cleanup. Reference slots are boxed only in the three cold
private snapshot enums; public loans and serialized topology remain unchanged.
The existing Controller memory-publication test keeps its identity and assertions
with the serial guard outside its current-thread runtime. Original grant-ordering
and EGC aggregation knowledge now distinguish administrative TLS selection from
namespaced Grant checks and historical evidence from current verification.

The first Resources focused build exposed two missed enum references in its
existing condition mirror. A separately approved two-line amendment corrected
those references without deleting assertions; the failed r1 log is retained.
The broader gRPC run then found an older matrix still accepting literal `core`
as a standard Service group alias. A separately reviewed test-only amendment kept
those inputs as negative cases, added an empty-group explicit-namespace success,
and isolated the invalid-kind case. Production target behavior was unchanged.
Both failed r1 logs remain available.

Fresh focused results: Common global TLS 7; Gateway backend context 3 and gRPC
routes 66 (r2); Resources references 67 (r2) and doctests 19 with 10 existing
ignored; Controller credential manager 50 and EGC handlers 18. All passed, with
no existing test identity removed. Latest Rust formatting check passed (fmt-r2);
agent-doc validation passed 1447/91/1050 (docs-r1). Production corrections are in
the user's independent commit `0f40e2708496e9aacafb0dc2fbd2cdd786ffa9e8`;
the final gRPC fixture correction and remaining documentation follow that commit.
Fresh independent result review and whole-workspace gates remain pending.
Snapshot and scoped delta are stored under
`/tmp/edgion-ref-checkpoint-before-20261007` and
`/tmp/edgion-ref-checkpoint-implementation.patch`; log prefix is
`/tmp/edgion-ref-checkpoint-`. Whole-workspace checks and the serial commit gate
remain with the primary coordinator; the next native slice has not started.

Checkpoint acceptance: fresh independent result reviewer
`/root/otlp_next_plan_inventory` returned C0 scoped PASS after verifying the
cumulative 4dbcc5495-to-user-0f40e2708 plus worktree, all corrective tests and the
279-path frozen digest. Runtime model is unknown; this is not an Astra result
claim. Workspace check-r2 and Clippy-r2 exited zero without warnings/errors;
format, docs-r3, SSA/metrics/Pingora/transform/reference/layout/tracing/English
r2 gates passed. CRD 28/eight and reference generator three-test inputs were
unchanged. Coordinator will commit only the twelve closing paths; the user's
snapshot is preserved and no push occurs. Whole-task coverage and conformance
remain pending, as tracked in 07-progress.md.

### R1 native Route implementation checkpoint

Independent Astra plan review approved the concrete actual-owner Route plan and
invalid-owner/denied-read amendments on baseline `8dab81bc6`. The initial scope was
13 production, 12 mirrored test and seven documentation paths; the coordinator
owns 07-progress.md separately. Only valid authored terminal loans mint Grant
queries. Invalid owners remain visible with rejected parent status and empty
derived execution state; denied targets retain dependencies without fresh
backend reads. Existing legal dependency cleanup and policy requeues remain.
Baseline and test identities: `/tmp/edgion-ref-native-before-8dab81bc`.
Implementation and focused verification are complete; independent final acceptance
and workspace gates remain with the coordinator. Gateway production, Grant-store,
worker, CRD and Cargo paths remain outside this slice.

R1 test-support amendment: independent Astra plan review approved the sole extra
`edgion-controller/tests/support/conf_mgr/sync_runtime/resource_processor/handlers/http_route.rs`
path (32 to 33 total). Its private fixture adapter preserves the full original rule
and explicit namespace while driving the actual owner-based AI resolver; no
production compatibility API is added. The first Controller helper compile failure
is retained in `/tmp/edgion-ref-native-helper-r1.log`; retry evidence follows below.

R1 fixture amendment: independent Astra review approved adding only the existing
`edgion-controller/tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/backend_tls_policy_test.rs`
path (34 total): its wrong-group assertion now matches the canonical exact error.
Two existing empty-group Route fixtures explicitly provide the required empty
group, preserving their rejection assertions. The new effective-status test uses
Tokio for its processor queue. Handler run 1 (648 passed, 4 failed) is retained in
`/tmp/edgion-ref-native-handlers-r1.log`; no policy or ExtensionRef production code
changes were required.

R1 Gateway fixture amendment: independent Astra review approved the sole extra
`edgion-gateway/tests/unit/routes/http/proxy_http/pg_upstream_peer_test.rs` path
(35 total). The shared valid-Service fixture now supplies required port 80; the
existing named-jump denial test asserts both compiled targets are Services before
its unchanged denial checks. The prior absent port compiled both slots Invalid;
`/tmp/edgion-ref-native-gateway-jump-denial-r1.log` preserves that failure. Gateway
production and the separate R2 selector/peer scope remain unchanged.


R1 review handoff: the three independently approved test-only amendments bring the
allowlist to 35 paths (13 production, 15 test/support, seven documentation).
The baseline comparison preserves all 294 existing test identities and adds 16;
no original test was removed. Final scope, SHA-256 manifest, patch and identity
comparison are under `/tmp/edgion-ref-native-before-8dab81bc`.

Focused PASS results (log prefix `/tmp/edgion-ref-native-`): Resources refs 70
(`resources-r1`), Resources doctests 22 with 10 existing ignored
(`resources-docs-r1`), Controller owner helper 6 (`helper-r3`), handlers 652
(`handlers-r2`), resource processor 909 (`resource-processor-r1`), Gateway peer
98 (`gateway-peer-r1`, includes named-jump and direct denial regressions), and
Gateway selector 11 (`gateway-weight-r2`), gRPC routes 66 (`gateway-grpc-r1`),
and selected-Service context guards three (`gateway-context-r1`). The first helper compile failures
(`helper-r1` and `helper-r2`), handler fixture failures (`handlers-r1`), and the
old missing-port jump fixture failure remain in their original logs. Formatting
passed through `fmt-r8`; layout passed with 999 mounted files. The coordinator
owns final workspace check/Clippy/guards, fresh independent result review, and
the serial commit gate. This handoff does not close the whole reference task.


R1 final-review correction: the independent Astra review and workspace Clippy
identified one `manual_let_else` warning in the terminal Grant-query projection.
The equivalent `let ... else` retains the same private-constructor invariant and
changes no authority or API. Fresh `resources-r2` passed 70 tests and
`resources-docs-r2` passed 22 with 10 existing ignored; `fmt-r9` passed. Workspace
check r1 passed in 1m 48s; Clippy r1 exited zero in 1m 40s but contained that one
warning (also repeated for lib tests), so it is not a warning-free gate. The
coordinator owns check/Clippy r2 and final acceptance. The current root AGENTS was
re-read; no applicable nested AGENTS exists. No other source or test changed in
this correction. The final manifest, patch and identity comparison are refreshed.


### R2a owning HTTP and GRPC backend execution

Independent Astra plan review approved the concrete ten-part plan and explicit
factory, policy-capture, and error-mapping amendments: 51 paths (19 production,
23 test/support, nine documentation). Baseline originals and all 347 existing
test identities are preserved under `/tmp/edgion-ref-r2a-before-0fea0a618`.
The coordinator owns 07-progress.md, final workspace gates and commits.

Actual Route effective fields produce private cold Service targets with canonical
key, policy identity and checked port section. Selected handles, sticky candidates,
DirectEndpoint and final peer construction borrow these targets. Invalid owners
invoke no plugin factory; invalid or denied slots retain weights but invoke no
backend factory, policy/affinity lookup or backend dispatch. Context is a checked
diagnostic projection, never a source of identity. Captured absence, old request
Arcs, retry budgets, direct endpoint proofs and the ordinary AI 503 remain intact.

Malformed HTTP descriptors, including invalid required Service ports, fail with
BackendUnsupportedKind/500 before side effects; valid Service runtime section or
endpoint failures retain their existing 503 mapping. This deliberately corrects
the old invalid-port 503 path under the vendored weighted-invalid contract.
The historical `invalid_backend_with_service_key_still_captures` test keeps its
identity and adversarial inputs but changes the unsafe capture expectation to
None and zero actual policy reads. No L4, Service.type/DNS, filter implementation,
plugin outbound, schema, Controller, Common, Grant-store or worker changes.
Focused verification and fresh independent result review remain pending.

R2a test amendment: independent Astra plan review approved correcting the existing
DIU (seven helper-driven cases) and Canary (four) standard-Service fixture port
from absent to 80. These cases exercise valid named jumps, namespace/weight or
denial behavior; their identities and other inputs/assertions remain intact. Each
module adds an explicit absent-port, matching-name Invalid case proving zero jump
publication and the plugin's existing onInvalid Reject 403. The frozen 51 paths
are unchanged. The sole AI detached TLS helper caller now uses its existing
logical handle; the unused detached Service branch is removed, leaving one native
Service section lookup and the same permitted EdgionBackend whole-target lookup.

R2a implementation verification: Resources refs 71 passed; Resources doctests
22 passed plus 10 existing ignored. Final Gateway HTTP 482, backends 275, GRPC 68,
AI HTTP/dispatch/model 16/30/4, DirectEndpoint 11, DIU 10 and Canary 14 passed.
Gateway doctests passed 8 plus 7 existing ignored, including two new private
compiled-identity compile-fail cases. HTTP wire tests distinguish malformed
500/denied 500/ordinary AI 503 and preserve gRPC HTTP 200/status 14. Factory,
policy-read and sticky lookup counters cover rejected inputs with positive
controls; borrowing/allocation tests pass. Unit layout passed 1000 mounted files,
and agent-doc validation passed 1447 files. Final reviewer requested only
owning-source comment corrections after these behavior tests; no logic changed.

Logs use `/tmp/edgion-ref-r2a-*.log`: retained library/test compile failures r1/r2
were repaired before test-check-r3 passed; HTTP-r1 passed 481 before the final
wire case, and HTTP-r2 passed 482 on the completed behavior. Gateway-doc-r1 hit
ENOSPC; the coordinator removed only stale ws3 incremental caches and the r2
retry passed. The immutable baseline remains unchanged. Final scope has 49 changed
files within the approved 51, preserving all 347 original test identities and
adding 11. Explicit assertion/input amendments are the unsafe policy capture
expectation and the two named-jump fixture ports described above. Full workspace
gates and independent final result review remain coordinator-owned and pending;
this is not closure of later L4, Service.type/DNS, filter or outbound slices.


### R2b approved L4 implementation

Independent Astra plan review approved 56 paths (19 production, 21 tests, 16
documents) at d37b890a8. The immutable baseline and 185 source test identities
are under `/tmp/edgion-ref-r2b-before-d37b890a8`; the coordinator also captured
5110 compiled Gateway test identities. The coordinator owns 07, final gates
and commits. No source outside this scope is authorized.

Actual TCP/TLS/UDP owners compile first-rule effective loans into private Service
targets and original weighted slots. Selected loans gate discovery, preserving
denial priority and making malformed descriptors terminal. TLS valid live
endpoint/connect retries remain; the shared actual-unit one-attempt helper has
resolver positive/negative controls, while final review checks both real loops'
terminal returns. Existing stream route stages still precede backend selection.

Invalid owners create no executable unit or publication demand. Full publication
uses successful units; partial publication removes every touched key and adds
only final surviving units. Shared same-key add clears before initialization.
Raw manager/publication keys preserve an omitted namespace as a bare name; only
canonical backend identity defaults the namespace. Old selected Arcs and pinned
UDP sessions retain their existing lifetime. No policy/TLS capture, new retry
policy, Controller/schema changes or Service.type/DNS work is included.

Focused verification passed on the completed source: Gateway runtime 12
(overlaps backends), TCP 24, UDP 51, TLS 83, common helpers 37, shared delta 2,
stream bridge 6, backends 278 and selector 25. Gateway doctests passed 11 with
seven existing ignored, including three new private-unit compile-fail cases.
Logs use `/tmp/edgion-ref-r2b-*-r1.log`; fmt-r2 passed. The initial library
check passed with one unused legacy-predicate warning; the obsolete raw helper
was removed before final tests. No focused test failed and no existing fixture
input/assertion correction was needed beyond the approved actual-owner wrappers.

All 185 original scoped source test identities remain (195 now); the actual
Gateway binary listing retains all 5110 original identities (5120 now). Ignored
attributes are unchanged. The 55-file delta stays inside the 56-path allowlist:
19 production, 20 test and 16 documentation files changed. Final scope hashes,
reproducible digest algorithm, baseline checks, patch and identity comparison are
under `/tmp/edgion-ref-r2b-final`. No baseline files were changed.

Resolver counts test the actual shared closure boundary; the TLS tests exercise
the actual one-attempt helper and retry-delay decision. They are not connector
or wire-conformance tests; independent result review must inspect both real
loops' terminal returns. UDP lifecycle tests preserve existing session semantics.
Workspace gates, fresh independent Astra result acceptance and commit remain
coordinator-owned; R2c and later reference consumers are still excluded.

The coordinator's first diff and unit-layout guards found trailing task-document
blank lines and four test-only accessors in production source. The EOF whitespace
was removed; independently approved accessor moves put the unchanged methods in
their existing mirrored test modules. The failed guard logs remain under the
`/tmp/edgion-ref-r2b-guard-*-r1.log` names. Fresh fmt-r3 and layout-r2 passed;
runtime/TCP/UDP/TLS-r2 passed 12/24/51/83. Earlier final artifacts are archived as
`/tmp/edgion-ref-r2b-final-r1` and `-r2`; the refreshed final snapshot supersedes
their hashes. No test identities, assertions, inputs or production behavior changed.

### R2c approved native ExternalName implementation

Independent Astra plan review approved 31 paths at 5cf413bbc: three production,
three unit-test files (one new mirror), seven DNS fixtures and 18 documents.
The immutable baseline is `/tmp/edgion-ref-r2c-before-5cf413bbc`; all 15 original
source test identities, 5120 compiled Gateway identities and ignored attributes
must remain. The seven fixture files change only ten positive native backend
kind values from ServiceExternalName to Service. The coordinator owns 07 and SCM.

A private Service selection captures one AvailableServiceProjection, checked
section and membership-matched runtime for one peer attempt. Native HTTP/gRPC
strategy comes from that actual Service's type; its hostname, annotations, port
and appProtocol remain coherent through DNS awaits. Endpoint and sticky selection
reuse that capture; later attempts recapture geometry while retaining existing
request policy/TLS ownership. Resolver callbacks borrow caller-owned inputs at
the actual query boundary. Standard descriptors stay strict. Existing DNS modes,
cache identity, SSRF, TLS, live runtime eligibility and SP behavior remain; L4
self-resolution, annotation authority (R6), custom outbound modes (R7), schemas,
Controller processing and new stores are excluded. Fresh verification evidence
is recorded below; independent final result review remains pending.

R2c amendment 01 received independent Astra plan PASS and coordinator execution
authorization: the effective allowlist is 32 paths. The existing
`edgion-gateway/tests/unit/backends/validation_test.rs` changes only its canonical
ServiceSectionKey import; all three additional original test identities and
bodies remain. Its immutable baseline is recorded separately under
`/tmp/edgion-ref-r2c-before-5cf413bbc/amendment-01`. The first focused compile
failed on that import and one new-test private-module import; a second attempt
found an Option/Result assertion mismatch in the new gRPC test. Both were corrected.
The original `/tmp` logs and compiled-identity listing were lost after a runtime
reset; earlier reported results are historical and their raw evidence is unavailable.
The baseline sources were recovered from immutable commit 5cf413bbc under
`tmp/resource-reference-contract/r2c-recovery-20261008`, reproducing the original
manifest SHA256 exactly. Fresh evidence uses
`tmp/resource-reference-contract/r2c-verification-20261008-r1`.

R2c fresh verification after recovery passed: native dispatch 12, complete
backends 290, peer 101, gRPC 68, DirectEndpoint-filtered 16, session-persistence
22, and Gateway doctests 11 with 7 existing ignored cases. Canonical prepare
completed successfully. The 18 original source test identities and ignored
attributes are unchanged; 12 new dispatch tests were added. Seven fixture files
contain exactly ten approved `ServiceExternalName` to `Service` kind-value
replacements; the import amendment changes no test body.

The direct canonical shared-topology run passed all four static DNS cases and
the complete DnsResolutionFlap scenario on one stack. Its unchanged scenario
awaits eight phases sequentially: failing, working, failing, working, bounded,
working, failing, working. The raw client reports scenario PASS and exit zero;
individual successful phases are intentionally silent, so their completion is
inferred from that unchanged fallible sequence, not a manufactured phase log.
The main runner's earlier macOS alias preflight did not pass; this is explicitly
a direct shared-topology result, not a full integration-runner or Kubernetes
conformance claim. Existing runner quality/capability, nofile, resource-sync,
single-Gateway and unknown-field gates passed. The sole owned stack was stopped;
the pre-existing token, current-run marker and both shared certificate trees
were restored exactly, with no matching product process left running. Original
credential backup copies were removed after restoration was verified; only
sanitized restoration metadata remains.

Fresh raw logs, binary/source hashes, ownership/restoration evidence and phase
source hashes are in `tmp/resource-reference-contract/r2c-verification-20261008-r1`.
The owned runtime logs remain under
`tmp/integration_testing/testing_20261008_084440`. Original historical `/tmp`
logs and the compiled identity listing remain unavailable. The coordinator is
reconstructing the original full compiled identity control from immutable HEAD;
required workspace gates and both independent final reviews remain pending.
