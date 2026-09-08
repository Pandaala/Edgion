---
name: credential-copy-ownership
description: Source-backed credential owners, release events, audit coverage, and zeroization limits across Controller, conf-sync, Gateway, and CLI.
knowledge_type: review-evidence
---

# Credential copy ownership and release boundaries

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
| BasicAuth | Borrow Secret UTF-8 then own trimmed fields; runtime clone takes raw users into prepared hashes; decoded password reuses String backing, separate username | Input config independent; request owns decoder output; `spawn_blocking` owns password and pinned user map until completion despite awaiter cancellation. Seen-map identity duplicates remain optional optimization |
| HMAC and shared conditions | Borrow YAML bytes; parser Value tree then final username/secret/metadata; immutable runtime Arc map | Per-entry/Secret parser locals drop; source config and verifier generations independent. HMAC state derives from borrowed key. Removing runtime map clone requires ownership redesign |
| KeyAuth | Borrow YAML bytes; typed parser map then configured metadata; runtime consumes raw keys into hashes | Request KeyGet String survives current filter/failure delay; future drop releases it. Parser field/seen-map duplication remains optional optimization |
| JWT | Borrow/trim Secret bytes once into final fields; constructor consumes group maps and creates DecodingKey | Unverified iss routing and verified claims are distinct; tokens/claims/request header copies drop with owners. Per-issuer serialized duplication and KeyGet/header ownership redesign remain tracked |
| OIDC | Final owned client/session secret, prepared TLS/cipher; refresh access/id token fields move into cookie; response and cookie DTOs remain owned | Forms/HTTP/decoded cookie/token values release with futures/requests. Access-token and refresh-result LRU TTLs bound validity, not destruction; lazy expired get/prune/eviction/clear/drop releases. TinyUFO expired lookup does not remove entries |
| LDAP | Decoded String with username/password ranges; separate bind DN; prepared TLS and SDK bind buffers | Request-local values release on future drop; timeout does not guarantee detached driver/SDK teardown. Unbind and driver have separate completion boundaries |
| ForwardAuth / Guardrail / AiGuard / templates | `ResolvedSecrets` owns configured substitutions; prepared client and retry template own request values; response capped by shared outbound exchange | Template retained across attempts/delays; response/request owners drop on completion/cancel, SDK owners separately. Logs/Debug must not format values |
| ExtProc | Independent tonic bidirectional client; resolved TLS becomes rustls client identity; Arc target runtime owns Channel, cache holds Weak only; request headers are copied into protobuf messages | Request stream owns opening future/inbound stream and four-message outbound queue. Per-exchange timeout/drop releases local owners; tonic transport, TLS and peer buffers remain independent. Shared HTTP response aggregation caps do not apply to this gRPC stream |
| JWE | Prepared cipher consumes decoded key; fallback moves selected bytes from owned Secret | Request encrypted/decrypted payload and forwarded identity have independent owners; source/crypto copies not erased by freeing one plaintext buffer |
| HeaderCertAuth | Borrow PEM while compiling CA pool; retained runtime config clears raw CA Secrets | Derived DER/subject pool persists with runtime; original config still independently owns Secret. Client certificate identity survives connection/request as designed |
| CredentialInjector | Prevalidated sensitive HeaderValue behind Arc; deferred attempt injection shares it | Validity deadline rejects use, not destruction; last runtime/request/attempt Arc drops. Managed memory-to-material temporary is a tracked optional copy reduction |
| OIDC/CSRF/Wasm Secret values | Shared helper returns borrowed UTF-8; final materialization owns exact bytes; Wasm status probe needs no String | Local fetch ends after resolution; published field lifetime follows generation. CSRF/OIDC whitespace behavior preserved |
| Shared TLS / LinkSys / AI | SecretSlot and TLS config own resolved Secrets; material produces selected CA/cert/key; clients may parse further | Full raw Secret projection into typed TLS material needs a shared consumer design; prepared dependencies and original resources are independent |
| ACME / DNS | ACME account final String and TLS private key material use Zeroizing; DNS resolver validates borrowed values | JSON Value syntax precheck and SDK crypto copies are not wiped; DNS SDK/process storage outside a proven erasure boundary. Syntax-tree removal needs equivalence checks |
| Gateway admin / CLI | Admin hashes borrowed trimmed token, stores digest; CLI file raw/trimmed Strings are Zeroizing and reqwest bearer header sensitive | Flags/env/files remain separate; header is independently owned by HTTP request. Issuance output is intentional product behavior |

Direct and nested formatting corrections are recorded in the
[credential Debug guard](../regression-guards/credential-material-debug-redaction.md).
Sensitive header provenance is recorded in the
[header-value rule](../mandatory-design/secret-header-values-must-be-sensitive.md).

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
  provider diagnostic boundary; see the [ACME DNS error redaction guard](../../acme/regression-guards/dns-provider-error-redaction.md)
  for the separate correction, verification, and remaining cleanup limits.

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

Optional unresolved copy reductions and shared-core questions are preserved with
source evidence in [credential ownership follow-ups](../../../../tasks/todo/auth-credential-ownership-followups.md).
The separate [OIDC response cap task](../../../../tasks/todo/oidc-auth-response-byte-caps.md)
tracks a body-boundedness defect; it is not accepted or claimed fixed by this audit.
Re-evaluate on new auth consumers, changed stores/serialization/retry tasks, new
Debug implementations, typed header conversions, or dependency behavior changes.

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

The original audit closes as completed investigation plus implemented scoped
corrections and transferred residual work, not removal of every avoidable copy or
every authentication defect. The linked pending tasks retain those claims.
Existing integration fixtures were unchanged; no full `cargo test --all` or
end-to-end topology was run. Configuration, authentication policy and hot-reload
semantics remain unchanged. The internal plugin development guide documents the
typed header API; operator-facing configuration guides require no change. Tests
verify behavior and metadata, not post-free memory erasure. No commit or push.
