---
name: credential-copy-ownership
description: Source-backed credential owners, release events, audit coverage, and zeroization limits across Controller, conf-sync, Gateway, and CLI.
knowledge_type: review-evidence
---

# Credential copy ownership and release boundaries

DNS-01 adapters and credential resolution were removed on 2026-09-09. DNS-specific observations below are historical and impose no restoration requirement.

## Conclusion and scope

D0036, originating in the B0067 BasicAuth audit, audits application-owned
authentication credentials and adjacent TLS, ACME,
managed-credential, and outbound-auth consumers. A release event is not an erasure
guarantee. Validity deadlines, cache TTLs, resource removal, request cancellation,
and Rust drop are distinct. No finite wall-clock destruction bound is established
for shared generations, idle history/cache entries, blocking work, SDK internals,
allocator residue, or network buffers. This page records observed ownership, not
acceptance of every retained copy or a universal no-secret-output guarantee.

Canonical requirements remain [design rules](../../../00-design-rules.md),
[log safety](../../../03-coding/01-log-safety.md), and the
[schema/transport contract](../../../01-architecture/05-resources/00-schema-transport-contract.md).
Authorized Secret CRUD and explicit CLI credential issuance necessarily carry real
credentials. Admin config projection, Debug, logs, and transport are separate
surfaces; real conf-sync data must never be replaced with admin-redacted data.

The `auth-credential-ownership-followups` review was closed without further code
changes on 2026-09-12 after an explicit user decision that the remaining copies do
not justify implementation work. That is a scoped value judgment, not proof that
the copies do not exist or that credentials are physically erased. The per-boundary
decision and re-evaluation triggers are recorded in the
[credential copy retention decision](../intentional-design/credential-copy-retention.md).

Earlier scoped corrections removed BasicAuth/HMAC identity warning fields and
borrowed Controller bearer lookup/HMAC YAML decoding. Their semantic guards remain
linked from the owning auth indices; their earlier verification was scoped, not
proof of this full inventory. The separate HMAC quick-start documentation
correction is maintained in the
[credential-list format guard](../../hmac-auth/regression-guards/credential-yaml-borrowed-decoding.md#credential-list-format-and-documentation-boundary).

## Controller and transport spine

| Owner / source symbol | Copy purpose and disposition | Release event and limits |
| --- | --- | --- |
| FS `FileSystemWatcher::prime` / etcd `EtcdWatcher::prime` | Input YAML owned until dispatch; the former permanent startup snapshots and two dispatch clones are removed | Subscribed phase entries move into events; runtime entry drops leftovers. Later phases keep their own entries; etcd revision remains. Re-prime/new watcher owns any restarted init |
| Kubernetes `resource_controller::run`; `LocalRawStore` | Decoded resources and `Arc<K>` form authoritative raw state; mutable worker clones isolate processing | Replace/remove/clear drops store reference; outstanding readers persist. Source files/etcd/K8s and dependency buffers are independent |
| `SecretHandler::parse` / `SecretStore::get` | Normalization transfers stringData bytes; initialization accumulator and live SecretStore provide ordered visibility; get clones outside lock | Accumulator taken at InitDone; store replace/remove/drop; fetched snapshot drops after resolver. Shared API snapshot redesign is tracked separately |
| `ResourceProcessor::process_work_item` / `process_resource_inner` | Previous/current resources support processing, status and after-publication work | Working generations drop after serial work/status IO; no universal completion deadline |
| `EventStore::apply_event`, `snapshot_owned`, `get_by_key` | Current map plus complete resumable event history; snapshots/getters own independent values | Replace/delete current map; ring overwrites by event count or clear/drop. Delete/history can retain credentials indefinitely when idle; no age expiry |
| `ConfigSyncServer::list_sync_cached` | Serialize borrowed resources into String; `Bytes::from` transfers allocation; cached/proto clones share bytes | Version mismatch rejects a hit but does not free it. Next successful/oversize fill, clear/drop and final Bytes owner release. There is **no TTL** |
| `ServerCache::watch` / `watch_json` and gRPC forwarding | History snapshot clones release lock before async sends; JSON serialization required; wrappers move Strings | Four message-count-bounded queue stages (100 each), plus in-flight sends. Drain/drop/task completion; blocked send and dependency frames are separate owners |
| Gateway `event_dispatch` | Bytes/String decode to typed resources; watch Value fields are moved into deserialization | Decode/error/publication ends local owner; serde/prost scratch erasure is not established |
| `CacheData::reset` / `insert` / `remove` | `Arc<T>` shares config with handlers; compressed events contain keys/change kinds only | Replace/remove drops cache reference; debounce is scheduling, not a hard destruction deadline |
| `PluginStore` / `CompiledPlugins` | Source `Arc<EdgionPlugins>` plus compiled runtime, atomically published snapshots | Last store/request/runtime reference; last-known-good fallback may intentionally keep prior generation after invalid update |

List cache reuse and ring recovery are existing mechanisms, not evidence of timed
credential revocation or physical destruction. Removing them to reduce residency
would require a separate sync/recovery design. The List response cap is checked
after serialization and is not a bound on every earlier allocation.

## Resolver and runtime inventory

Every `handlers/plugins/resolve_credential` sibling was inspected: BasicAuth,
KeyAuth, JWT, HMAC, OIDC, LDAP, HeaderCertAuth, CSRF, JWE, ForwardAuth, ExtProc,
Guardrail, AiGuard, CredentialInjector, Wasm, and their shared helper. All material
resolvers under `edgion-resources/src/credentials` were inspected, including DNS.

| Consumer | Required owners / implemented reductions | Release / residual classification |
| --- | --- | --- |
| BasicAuth | Borrow Secret UTF-8 then own trimmed fields; runtime clone takes raw users into prepared hashes; decoded password reuses String backing, separate username | Input config independent; request owns decoder output; `spawn_blocking` owns password and pinned user map until completion despite awaiter cancellation. The parser's extra username/group seen map is retained as a low-value cleanup candidate, not a password copy or security defect |
| HMAC and shared conditions | Borrow YAML bytes; parser Value tree then final username/secret/metadata; immutable runtime Arc map | Per-entry/Secret parser locals drop; source config and verifier generations independent. The temporary selected-scalar copies and username/group seen map are retained after the value review; the runtime map is a required independent owner |
| KeyAuth | Borrow YAML bytes; move the selected key and allowed metadata out of each typed parser entry; the result map also owns first-wins detection; runtime consumes raw keys into hashes | No separate parser field clones or seen-map key copies. Source Secret/config owners remain independent. Request KeyGet String survives current filter/failure delay; future drop releases it |
| JWT | Borrow/trim Secret bytes once into final fields; constructor consumes group maps and creates DecodingKey; request extraction borrows the header before owning the selected token | Unverified iss routing and verified claims are distinct; tokens/claims drop with owners. Per-issuer serialized duplication and the selected request token remain accepted scoped owners unless scale or hot-path evidence changes |
| OIDC | Final owned client/session secret, prepared TLS/cipher; refresh access/id token fields move into cookie; response and cookie DTOs remain owned | Forms/HTTP/decoded cookie/token values release with futures/requests. Access-token and refresh-result LRU TTLs bound validity, not destruction; lazy expired get/prune/eviction/clear/drop releases. TinyUFO expired lookup does not remove entries |
| LDAP | Decoded String with username/password ranges; separate bind DN; prepared TLS and SDK bind buffers | Request-local values release on future drop; timeout does not guarantee detached driver/SDK teardown. Unbind and driver have separate completion boundaries |
| ForwardAuth / Guardrail / AiGuard / templates | `ResolvedSecrets` owns configured substitutions; prepared client and retry template own request values; response capped by shared outbound exchange | Template retained across attempts/delays; response/request owners drop on completion/cancel, SDK owners separately. Logs/Debug must not format values |
| ExtProc | Independent tonic bidirectional client; resolved TLS becomes rustls client identity; Arc target runtime owns Channel, cache holds Weak only; request headers are copied into protobuf messages | Request stream owns opening future/inbound stream and four-message outbound queue. Per-exchange timeout/drop releases local owners; tonic transport, TLS and peer buffers remain independent. Shared HTTP response aggregation caps do not apply to this gRPC stream |
| JWE | Controller-resolved transport value decodes through zeroizing buffers into one prepared cipher; Gateway has no local Secret fallback | Request encrypted/decrypted payload and forwarded identity have independent owners; source/crypto copies and the cipher schedule remain separate owners |
| HeaderCertAuth | Borrow PEM while compiling CA pool; retained runtime config clears raw CA Secrets | Derived DER/subject pool persists with runtime; original config still independently owns Secret. Client certificate identity survives connection/request as designed |
| CredentialInjector | Prevalidated sensitive HeaderValue behind Arc; deferred attempt injection shares it | Validity deadline rejects use, not destruction; last runtime/request/attempt Arc drops. Publication, conf-sync, runtime-header and attempt owners remain separate to preserve retry and injection semantics |
| OIDC/CSRF/Wasm Secret values | Shared helper returns borrowed UTF-8; final materialization owns exact bytes; Wasm status probe needs no String | Local fetch ends after resolution; published field lifetime follows generation. CSRF/OIDC whitespace behavior preserved |
| Shared TLS / LinkSys / AI | SecretSlot and TLS config own resolved Secrets; material produces selected CA/cert/key; clients may parse further | Full raw Secret projection is retained because replacing it requires a shared transport/consumer redesign; reconsider only with scale or exposure evidence. Prepared dependencies and original resources remain independent |
| ACME | ACME account final String and TLS private key material use Zeroizing; EAB fetch and decode create cold-path application and SDK owners | The EAB copies are retained because account registration is rare and removing one local owner would not remove the source Secret or SDK state. JSON/crypto scratch is not proven wiped |
| Gateway admin / CLI | Admin hashes borrowed trimmed token, stores digest; CLI file raw/trimmed Strings are Zeroizing and reqwest bearer header sensitive | Flags/env/files remain separate; header is independently owned by HTTP request. Issuance output is intentional product behavior |

Direct and nested formatting corrections are recorded in the
[credential Debug guard](../regression-guards/credential-material-debug-redaction.md).
Sensitive header provenance is recorded in the
[header-value rule](../mandatory-design/secret-header-values-must-be-sensitive.md).
The `auth-borrowed-authorization-header` / D0035 shared-accessor follow-up is
already implemented; its current consumer scope and residual ownership limits
are maintained in the
[borrowed request-header guard](../../plugin-runtime/regression-guards/borrowed-request-header-access.md).

## Adjacent managed credentials, SDKs, and ACME

- Managed OAuth2 `BootstrapCredentials` owns a Zeroizing client secret while the
  source Secret remains independent. The uniquely owned percent-encoded secret
  is now wrapped immediately without another allocation. Joined/base64/header
  scratch and bounded response aggregation already use Zeroizing; SDK chunks and
  final HeaderValue do not. Canonical BearerResolver validation still creates its
  own validated token. `RetainedPublication` keeps and clones the issued token for
  publication retries without reissuing; removing it would change correctness.
- `GeneratedSecretEntry` owns Zeroizing bytes behind Arc. Rotation/expiry/removal
  drops store references; the final external Arc drop wipes that entry. Expiry
  scanning and cancellation polling are release events, not an exact destruction
  deadline. Persistent Kubernetes publication creates independent durable copies.
- LinkSys Redis/etcd/Kafka client configs and health/reconnect tasks retain owned
  authentication state for reconnect. Runtime store replacement drops one owner;
  existing clients/tasks and SDK buffers survive independently. Elasticsearch
  request authorization now preserves sensitive HeaderValue metadata for all
  three schemes. Webhook retry templates retain expanded values across attempts.
- ACME account JSON decode moves cloned Vec storage into String and Zeroizing on
  success; SDK account/signing state and original Secret are independent. EAB
  extraction and DNS provider construction have optional cold-path copy reductions.
  DNS providers retain credentials through create/propagation/cleanup. Detached
  ACME flows are generation-gated for publication, not aborted at resource removal;
  credential release follows flow completion/cancellation/runtime shutdown.
- At the time of this ownership audit, DNS response aggregation was capped but
  arbitrary provider error text reached diagnostics. This defect was deferred
  by the ownership audit and subsequently corrected at the DNS
  provider diagnostic boundary; see the [ACME DNS error redaction guard](../../acme/superseded/dns01-retired.md)
  for the separate correction, verification, and remaining cleanup limits.

## KeyAuth parser ownership follow-up

`auth-credential-ownership-followups` (D0036 follow-up), KeyAuth parser boundary:
**change** approved as a low-priority copy reduction. The Controller's
`handlers/plugins/resolve_credential/key_auth.rs::resolve_group` removes the
configured `key_field` from each owned YAML entry and consumes allowed metadata
into `KeyMetadata.headers`. `first_wins_insert` uses the result map's entry API;
an occupied entry lends `KeyMetadata.group` to the unchanged duplicate warning.
The [first-wins rule](../../key-auth/mandatory-design/dup-key-first-wins.md) remains
the authority for duplicate handling and credential-free diagnostics.

| Owner | Purpose and change | Release event and remaining owners |
| --- | --- | --- |
| Parsed YAML entry | Owns decoded strings; selected key and allowed fields move instead of being cloned | Rejected fields and missing-key entries drop during consumption; remaining list entries await their iteration |
| Former `seen` map | Duplicated every accepted raw key and group solely for duplicate detection; removed | No separate buffer to retain until the end of this resolution pass |
| `all_keys` / `KeyAuthConfig.resolved_keys` | One result map owns accepted keys and metadata, and supplies duplicate detection | Publication transfers the map into the config; independent source/config/sync generations follow their own last owners |

This removes an O(total accepted key bytes) temporary owner and transient field
copies from configuration parsing. It does not affect Gateway request work,
redesign shared configuration, wipe freed memory, or establish a destruction
deadline. Original Secret snapshots, serialized resources and Gateway source
configurations remain independent. Header-map allocation is still required;
this is not an allocation-free parser or a measured performance claim.

The focused resolver tests cover distinct keys, first-wins group reporting,
metadata retention, custom key selection (including its exclusion from forwarded
metadata), preserved whitespace/empty-key parsing behavior, safe duplicate warnings,
Secret reference registration, and replacement/removal on re-resolution. This is
an in-process resolver regression, not a live watch or end-to-end hot-reload test.
Fresh focused execution passed: `cargo test -p edgion-controller --lib`
with filters `resolve_credential::key_auth::tests` (3 tests),
`parse_clears_injected_auth_credentials` (2 tests), and
`credential_parse_warnings_omit_yaml_scalars_and_jwt_identities` (1 test).
Independent read-only review returned LGTM. `cargo fmt --all`,
`cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets`,
SSA-force, metrics-inventory, Pingora-source-policy, and `git diff --check` passed.
Clippy emitted existing warnings outside the changed file; this is not a
warning-free workspace result. The Gateway API architecture, TLSRoute feature
page, and feature index separately document the v1alpha3 fallback debt; the
KeyAuth change does not alter it. Re-evaluate if parser shapes,
duplicate policy, metadata selection, or result-map ownership changes. The remaining
ownership candidates were subsequently assessed and closed without code changes as
recorded below.

The explicit keep decisions, accepted costs, alternatives, Envoy comparison, and
re-evaluation triggers for these residual boundaries are maintained in the
[credential copy retention decision](../intentional-design/credential-copy-retention.md).

## Zeroization decision

Use existing Zeroizing only where an owned buffer has a known last owner, including
CLI file tokens, OAuth encoding scratch, and final private-key material. Do not wipe shared config/cache
generations in place. No broad wrapper migration is made: source Secret stores,
serialized copies, header clones, crypto schedules, dependency scratch buffers and
transport frames remain independently owned. A successful `String::from_utf8(Vec)`
transfers allocation rather than copying; `Zeroizing::new` also moves ownership.
An invalid UTF-8 error can own the rejected Vec until drop. Tests of formatting,
owner release or wrapper construction do not measure post-free allocator erasure.

## Coverage, unresolved work, and re-evaluation

Source evidence spans Controller conf_center raw watchers/resource controllers,
sync_runtime processing and resolvers, conf_sync server/cache, Gateway client/cache,
plugin store and the runtimes above, shared resources/redaction/materials, and CLI
token/client auth. This is application source evidence, not an exhaustive audit of
third-party library allocations or deployed log/storage history.

The residual optional copy reductions and shared-core questions were closed by the
[no-code value decision](../intentional-design/credential-copy-retention.md); they
are not unresolved implementation commitments.
The body-boundedness defect deferred by this audit is corrected separately in the
[OIDC authentication response cap guard](../../oidc/regression-guards/authentication-response-byte-caps.md).
That correction does not establish credential erasure or full public-error redaction.
Re-evaluate on new auth consumers, changed stores/serialization/retry tasks, new
Debug implementations, typed header conversions, or dependency behavior changes.

The recovery task `credential-audit-missing-followups` was superseded by the four
tracked original task files. Inspection at that recovery review confirmed their presence;
the claim that they are absent from both the checkout and available Git history
no longer applies. The HMAC quick-start and ACME DNS diagnostic corrections have
the semantic guards linked above, although their original task metadata still
says pending. The credential ownership follow-up remained open at that recovery
review and was later closed by the scoped no-code decision above. At recovery
review time, token exchange and refresh still called `Response::json` directly;
the later response-cap correction, including introspection, is recorded in the
OIDC guard linked above.
Closing the recovery record does not close those follow-ups, reconcile their
individual lifecycle metadata, or claim new execution of their historical tests.
Re-evaluate this recovery conclusion if those task files or semantic links disappear.

Recovery-record closure checks: `cargo fmt --all`, workspace/all-targets check
and clippy, agent-doc validation, SSA-force, metrics-inventory, Pingora-source-policy,
and `git diff --check` passed. Clippy reported existing Rust warnings; this
documentation-only closure changes no runtime code and reruns no runtime tests.
Agent-doc validation initially found a separate missing AliDNS selection task;
its pending record was reconstructed from current source before validation passed.

## Verification

Fresh verification for the completed D0036 investigation and scoped corrections:

- Resources: 28 `redact` tests and 6 `credentials::materials::tests` passed.
- Controller: 87 EdgionPlugins handler tests, 25 resolver tests, 23 FS watcher
  tests, 2 etcd watcher tests, and 1 OAuth Basic-encoding regression passed.
- Gateway: 463 selected test executions passed across common outbound/auth,
  OIDC, JWT, JWE, BasicAuth, KeyAuth, HMAC, HeaderCertAuth, LDAP, the real typed
  session adapter, client certificate Debug, Elasticsearch and admin auth.
- CLI: 89 library CLI tests passed, including static bearer bytes/sensitivity.
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets` passed. Clippy retained existing
  workspace warnings; none of its diagnostics point to added diff lines. This
  is not a warning-free workspace result.
- Agent-doc validation, SSA-force, metrics-inventory, Pingora-source-policy and
  `git diff --check` passed. The current lockfile/policy uses the complete fork
  family on `edgion_v4`; stale repository prose is not evidence of another build.
- Independent read-only review: LGTM for implementation, knowledge and transfer
  of unresolved work. It corrected initial JWT selector and ExtProc lifecycle
  descriptions before approval.

The original audit closed as completed investigation plus implemented scoped
corrections and transfer of then-residual work, not removal of every avoidable copy
or every authentication defect. The later residual disposition is maintained in the
[credential copy retention decision](../intentional-design/credential-copy-retention.md),
not an active ownership task.
Existing integration fixtures were unchanged; no full `cargo test --all` or
end-to-end topology was run. Configuration, authentication policy and hot-reload
semantics remain unchanged. The internal plugin development guide documents the
typed header API; operator-facing configuration guides require no change. Tests
verify behavior and metadata, not post-free memory erasure. No commit or push.
