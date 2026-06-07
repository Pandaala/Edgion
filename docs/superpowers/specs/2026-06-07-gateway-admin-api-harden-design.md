# Gateway Admin API Hardening — Design

> Date: 2026-06-07
> Source task: `tasks/todo/gateway-admin-api-harden.md` (B-direction hardening)
> Related findings: `tasks/review-2/types-misc-01-admin-api-bind-0000-no-auth.md` (P1/Security, closed → todo);
> `fr-grt-01` (`/configclient/EdgionTls` leaked TLS private keys, fixed via EdgionTls scrub).
> Related todo: `tasks/todo/admin-api-https-and-access-control.md` (that one is the **Controller/Center**
> admin API; this design is the **Gateway** data-plane admin API — different binary, different problem).
>
> This spec was hardened by an adversarial multi-agent review (2026-06-07): the secret inventory was
> incomplete, the endpoint-gating plan would have broken a shipped CLI consumer, and several mechanism
> details needed tightening. All findings are folded in below.

## Background

The Gateway data-plane Admin API (`src/core/gateway/api/mod.rs`, spawned by
`EdgionGatewayCli::start_auxiliary_services` in `src/core/gateway/cli/mod.rs`) exposed a P1 security
surface: the `/configclient/{kind}` and `/configclient/{kind}/list` endpoints serialize the entire
synced data-plane configuration — **including resolved Secret bytes** (TLS private keys, JWT signing
keys, link-sys credentials) pushed down by `conf_sync` — with no authentication.

### Already shipped before this design (do NOT re-do)

Two of the original three tasks, plus an architectural improvement beyond what the task file planned,
have already landed:

- **Loopback default (Task 2):** `DEFAULT_GATEWAY_ADMIN_LISTEN = 127.0.0.1:5810`
  (`src/core/gateway/cli/config.rs`). Unconfigured operators no longer expose the admin API
  cluster-wide. Unit tests `test_admin_listen_defaults_to_loopback` /
  `test_admin_listen_explicit_value_is_respected` guard it.
- **Probe-port split (better than the original Task 1 plan):** instead of "make the always-mounted
  router safe enough to bind `0.0.0.0`", the implementer adopted the Istio-style separation. A
  dedicated **probe port** `DEFAULT_GATEWAY_PROBE_LISTEN = 0.0.0.0:28000` serves only the
  non-sensitive `/health` + `/ready` (`create_probe_router`). The admin port stays loopback. The
  shipped deployment manifests (`deploy/kubernetes/gateway/{deployment,configmap}.yaml`) already point
  the kubelet `httpGet` probes at 28000; the admin port is never probed. Unit tests
  `test_probe_listen_*` guard it.
- **EdgionTls scrub (fr-grt-01):** `EdgionTlsSpec.secret` and `ClientAuthConfig.ca_secret` are nulled
  before admin serialization via the imperative `scrub_edgion_tls_secrets` +
  `to_json_vec_edgion_tls`/`find_edgion_tls` special-casing in `api/mod.rs`. This design generalizes
  that one-off into the uniform mechanism below and removes the special-casing.

### What remains (this design)

- **Comprehensive secret redaction (Task 3, generalized).** `/configclient` still serializes resolved
  secrets for every resource kind other than EdgionTls (Gateway listener serving keys, EdgionPlugins
  auth material, LinkSys credentials, BackendTLSPolicy). The EdgionTls fix covered one kind out of
  many.
- **Endpoint surface reduction (Task 1, revised to "Plan B" — see decision below).** Move only the
  test-only `/api/v1/server-info` into the testing-gated router; **keep `/configclient` mounted** but
  make its output mandatorily redacted.

### Residual risk being closed

In the shipped config the admin port is loopback-only, so `/configclient` is not cluster-reachable.
The remaining exposure is the narrow case where an operator explicitly sets `admin_listen: 0.0.0.0`
(habit, or copying an old configmap). Mandatory redaction (Part B) removes the secret-leak entirely
regardless of bind address; loopback default + the probe-port split keep the network surface minimal.
This matches Envoy `config_dump`'s mandatory secret redaction model.

## Key design decision: keep `/configclient` mounted, rely on redaction (Plan B)

The original Task 1 plan was to move `/configclient` behind the integration-testing-mode gate. The
review found that this would break a **shipped production consumer**: `edgion-cli get <kind>
--target client` hits the gateway `/configclient/{kind}/list` and `/configclient/{kind}` on port 5810
(`src/core/ctl/cli/client.rs:310,386`). Gating those routes would make `edgion-cli -t client` return
404 against any production-mode gateway.

Because Part B makes `/configclient` output **mandatorily redacted** (no secret can leave through it),
the security justification for gating it collapses to mere attack-surface/fingerprint reduction —
which is not worth losing a shipped debug capability. Therefore:

- **`/configclient/{kind}` and `/configclient/{kind}/list`: stay always-mounted**, with mandatory
  redaction applied unconditionally. `edgion-cli -t client` continues to work in production and
  returns the resource structure with secret fields shown as `"[redacted]"`. The admin port remains
  loopback by default + (now) leak-free, so this is a localhost-only, secret-free introspection path.
- **`/api/v1/server-info`: move into `create_testing_router`.** The review confirmed the gateway's
  `server-info` is consumed **only** by integration test scripts (all of which run with
  `--integration-testing-mode`); no edgion-cli/production/probe consumer exists. Gating it removes a
  test-only fingerprint endpoint from production with zero consumer impact.

(`/health` + `/ready` remain on the dedicated probe port 28000, untouched.)

## Part A — endpoint surface reduction

Files: `src/core/gateway/api/mod.rs` (`create_admin_router` / `create_testing_router`).

- Remove `GET /api/v1/server-info` from the always-mounted `create_admin_router`; add it to
  `create_testing_router` (already gated by `crate::core::common::config::is_integration_testing_mode()`).
  `get_server_info` takes `State<Arc<ConfigClient>>`, so add the route **before** the
  `.with_state(config_client)` call in `create_testing_router` (group it with `store_stats`) so its
  `State` extractor is satisfied. Same `Arc` clone — no shared-state conflict.
- Leave `GET /configclient/{kind}` and `GET /configclient/{kind}/list` on `create_admin_router`
  (always mounted), now serialized through the redaction choke point (Part B).
- `create_probe_router` (28000) is untouched.

Net production effect: the admin port (loopback 5810) serves `/configclient` with redacted output;
`/api/v1/server-info` is testing-mode-only.

## Part B — mandatory admin-scope secret redaction (serde field adapter)

The project already has a blessed idiom for "do not emit sensitive data during serialization": the
access-log sensitive subsystem (`src/types/sensitive.rs`) annotates fields with
`#[serde(serialize_with = "...masked_opt::serialize")]`, where the adapter consults an ambient mode.
Part B extends this idiom rather than inventing a parallel mechanism (proc-macro derive) or an
imperative clone-and-scrub pass.

**Key distinction between the two concerns (drives the separate ambient state):**

| Concern | Governance | Mechanism |
|---|---|---|
| Access-log masking | Operator-configurable (`SENSITIVE_MODE` = Off/Replace/Wrap) | `sensitive.rs` |
| Admin config-dump redaction | **Mandatory, always-on, no config knob** | `redact.rs` (new) |

Admin redaction **must not reuse `SENSITIVE_MODE`** — otherwise an operator setting access-log masking
to Off would also disable the security redaction, which is unacceptable. The two share the *idiom*
(field `serialize_with` adapter + ambient state) but use *independent* ambient state.

### Why serde-layer redaction is viable here but not globally

`conf_sync` transport and the admin API use the **same** `#[derive(Serialize)]` on the resource
structs in `src/types/resources/`, but they run in **different binaries**:

- **Transport serialization runs in the CONTROLLER binary**, in the config-sync cache server:
  `serialize_snapshot_json()` (`src/core/controller/conf_sync/cache_server/store.rs:134`,
  `serde_json::to_string`) for the list/snapshot path, and
  `src/core/controller/conf_sync/cache_server/cache.rs:318` for the watch-event path. This must emit
  **real** secret bytes so the Gateway can rebuild them. The controller never constructs the gateway's
  redaction guard, so it always emits real values.
  (Note: the earlier draft cited `conf_sync/conf_client/config_client.rs:386-407` /
  `ResourceList::to_json` as the transport serializer — that was wrong; that file is the gateway-side
  *receiver*. Corrected here.)
- **Admin serialization runs in the GATEWAY binary**, in `list_json_dyn`
  (`src/core/gateway/conf_sync/cache_client/cache.rs:232`, `serde_json::to_value`) reached from the
  `list_resources` / `get_resource` handlers.

So the redaction is gated on an **ambient "admin output" scope** that exists only in the gateway admin
handlers. Default behavior (no scope) emits the real value, so transport — in a *separate process* —
is structurally incapable of being redacted. We cannot instead make the foreign `k8s_openapi::Secret`
type self-redact (orphan rule, and it would break transport).

### New module `src/types/redact.rs`

1. **Scope guard** — a thread-local flag plus an RAII `AdminRedactGuard` that sets it on construction
   and clears it on `Drop`; a `pub fn redact_active() -> bool`. There is **no** Off/Replace/Wrap and
   **no** config: while the scope is active, redaction is unconditional. RAII + unwind semantics
   guarantee the flag is cleared even if a handler panics (the build uses `panic = unwind`), so a
   panic cannot poison subsequent requests on the same worker thread.

2. **Two serde adapters** (covering both `Option<T>` and bare secret fields):

   ```rust
   /// For `Option<T>` secret fields (the common case).
   /// `#[serde(serialize_with = "crate::types::redact::redact_opt::serialize")]`
   pub mod redact_opt {
       use serde::{Serialize, Serializer};
       pub fn serialize<T: Serialize, S: Serializer>(v: &Option<T>, s: S) -> Result<S::Ok, S::Error> {
           if super::redact_active() { s.serialize_str("[redacted]") } else { v.serialize(s) }
       }
   }

   /// For bare (non-Option) secret fields, e.g. `CsrfConfig.key: String`.
   /// `#[serde(serialize_with = "crate::types::redact::redact_required::serialize")]`
   pub mod redact_required {
       use serde::{Serialize, Serializer};
       pub fn serialize<T: Serialize, S: Serializer>(v: &T, s: S) -> Result<S::Ok, S::Error> {
           if super::redact_active() { s.serialize_str("[redacted]") } else { v.serialize(s) }
       }
   }
   ```

   The generic `serialize_with` form is valid serde (it monomorphizes per field type, exactly like
   the existing `masked_opt::serialize`). In the redact branch the adapter emits the scalar
   `"[redacted]"` and **never recurses into the value**, so a single annotation on the top-level field
   collapses the entire subtree (including any nested secret sub-fields) to one string — strictly
   safer, and it means we annotate only the outermost container. `skip_serializing_if = "Option::is_none"`
   is retained on `Option` fields: `None` → omitted (adapter never runs); `Some` → `"[redacted]"` in
   scope / real value otherwise (serde evaluates `skip_serializing_if` before the adapter — no ordering
   gotcha).

   No re-entrancy hazard: unlike `sensitive.rs::apply_sensitive` (which holds a `RefCell` borrow
   across its closure in Wrap mode), these adapters only do a scalar thread-local read, then either
   emit a string or recurse normally with the flag unchanged.

### Field annotations — complete inventory

Annotate each resolved/plaintext secret field with the adapter (keep existing `skip_serializing_if`).
Where a shared secret-holder type backs multiple resources, annotate it **once**. The list below is
the review-corrected, complete set; the implementer must still reconcile it against the code and
satisfy the invariant: *every field holding resolved or plaintext secret material that reaches the
`/configclient` serializer carries a redaction adapter.*

**Shared holders (annotate once each):**
- `SecretSlot.secret: Option<Secret>` — `src/types/resources/common/secret_slot.rs`. Covers LinkSys
  Redis/Etcd/Elasticsearch/Webhook `auth.*` and is reused widely.
- `TlsCerts.secret: Option<Secret>` — `src/types/resources/common/tls.rs`. Covers all `tls.certs.*`.

**EdgionTls** (`src/types/resources/edgion_tls.rs`): `spec.secret`, `spec.client_auth.ca_secret`
(both `Option<Secret>`). **Remove** the imperative EdgionTls special-casing in `api/mod.rs` and route
EdgionTls through the uniform path. The actual edit sites (verified against current code — there is
**no** `find_edgion_tls`; `list_client_resources`/`get_client_resource` are plain `fn`s, not macros):
- Delete the `ResourceKind::EdgionTls => to_json_vec_edgion_tls(client.edgion_tls().list().data)` arm
  in `list_client_resources` (`api/mod.rs:106`) so EdgionTls falls through to the generic
  `get_dyn_cache(kind).list_json_dyn()` path like every other kind.
- Delete the now-unused `to_json_vec_edgion_tls` (`api/mod.rs:156`) and `scrub_edgion_tls_secrets`.
- The typed `client.edgion_tls()` accessor stays (still used by a `#[cfg(test)]` test). After deleting
  the two functions, the top-level `use crate::types::resources::edgion_tls::EdgionTls;` (`api/mod.rs:2`)
  is referenced **only** from `#[cfg(test)]` code, so a non-test build would emit `unused_imports`
  (a hard error under `-D warnings`). Fix: **move that `use` into the test module's use block**
  (alongside `EdgionTlsSpec` etc.), do not merely delete it.
- Update the now-false doc comments that assert "EdgionTls must scrub via the typed path":
  `api/mod.rs` (the comments around the EdgionTls accessor and `to_json_vec_edgion_tls`),
  `conf_sync/conf_client/config_client.rs` (the `edgion_tls()` accessor doc), and
  `conf_sync/cache_client/cache.rs` (the `list_json_dyn` "callers that expose secret-bearing kinds must
  scrub through their typed path" note). Left unchanged they actively mislead, since redaction now
  happens uniformly at the serde layer.

**Gateway** (`src/types/resources/gateway.rs`) — note the review-added listener fields:
- `spec.listeners[].tls.secrets: Option<Vec<Secret>>` — **listener serving cert + private key
  (`tls.key`)**. HIGH: omitted from the first draft; the most important addition.
- `spec.listeners[].tls.resolved_frontend_ca_secrets: Option<Vec<Secret>>` — frontend mTLS CA.
- `spec.tls.backend.resolved_client_certificate: Option<Secret>` (`GatewayBackendTLS`).

**BackendTLSPolicy** (v1 + v1alpha3, `src/types/resources/backend_tls_policy.rs`):
`resolved_ca_certificates: Option<Vec<Secret>>`, `resolved_client_certificate: Option<Secret>`.

**EdgionPlugins** (`src/types/resources/edgion_plugins/plugin_configs/`):
- JweDecrypt `resolved_credential: Option<ResolvedJweCredential>`
- JwtAuth `resolved_groups_by_iss: Option<HashMap<String, ResolvedJwtIssuerGroup>>`
- BasicAuth `resolved_users: Option<HashMap<String, ResolvedBasicAuthUser>>`
- HmacAuth `resolved_credentials: Option<HashMap<String, HmacCredential>>`
- OpenidConnect `resolved_client_secret: Option<String>` + `resolved_session_secret: Option<String>`
- **KeyAuth `resolved_keys: Option<HashMap<String, KeyMetadata>>`** — HIGH (review-added): the
  **HashMap key is the plaintext API key**. Annotating the `Option<HashMap>` as a unit redacts it.
- **HeaderCertAuth `resolved_ca_secrets: Option<Vec<Secret>>`** (review-added, CA material).
- **Csrf `CsrfConfig.key: String`** (`plugin_configs/csrf.rs:27`, review-added) — a bare `String`, not
  `Option`; use `redact_required`. (Note: `CsrfConfig`'s own `test_serde_serialization` asserts the key
  is present with no guard active — it relies on `redact_required` defaulting to the real value, which
  it does, so that test still passes.)

**LinkSys** (`src/types/resources/link_sys/`): covered by the shared `SecretSlot` / `TlsCerts`
annotations above, **except**:
- **`RedisSentinel.password: Option<String>`** (review-added) — an inline Sentinel password that does
  **not** go through `SecretSlot`; must be annotated directly. (Corrects the earlier "shared holders
  cover all of LinkSys" claim.)

**EdgionAcme** (`src/types/resources/edgion_acme.rs`) — review-added; the second review round confirmed
EdgionAcme **is** gateway-synced (`config_client.rs:263`,
`ResourceKind::EdgionAcme => Some(&self.edgion_acme)`) and reachable at `/configclient/EdgionAcme`.
(The original task file's "EdgionAcme is not synced to gateway" assumption is outdated.) Annotate the
`Option` parent `spec.external_account_binding: Option<AcmeExternalAccountBinding>`
(`edgion_acme.rs:281`) with `redact_opt` — this collapses the whole EAB, including the inline plaintext
`hmac_key: String` (`edgion_acme.rs:155`, the ZeroSSL/EAB HMAC credential) and the public `key_id`, to
`"[redacted]"`. (The ACME account private key is **not** inlined — only `private_key_secret_ref` (a
name) is synced — so no further field is needed. `ActiveHttpChallenge.token` / `key_authorization` in
status are the publicly-served HTTP-01 response, not secrets.)

**PluginMetaData** (`src/types/resources/plugin_metadata/`) — review-added. The resource carries a
`MetaDataEntry` enum (`mod.rs`) with, among others, two relevant variants:
- **`KeyList(KeyListData)`** → `MetaDataItem.key: String` (`key_list.rs:63`) — key values / regex used
  by **KeyAuth**, i.e. the set of valid **API keys**. This is credential material of the same class as
  the already-redacted `KeyAuth.resolved_keys`. **Redact** it with `redact_required` (bare `String`).
- **`IpList(IpListData)`** → `IpGroup.cidrs: Vec<String>` — CIDR strings for IP filtering. **Not**
  secret; leave visible so IpRestriction rule introspection still works. (CIDRs live here, NOT in
  `MetaDataItem.key` — the earlier "dual-use" framing conflated the two variants.)

Residual cost: a KeyList used for non-credential string matching would also render its match values as
`"[redacted]"` — accepted, since KeyList's primary use is API keys and credential-safe-by-default is
the right call. Both are gateway admin `/configclient/PluginMetaData` config output and are unrelated
to the access-log subsystem. (`MetaDataEntry`'s other three variants — `ClusterRegionRoute`,
`ServiceRegionRoute`, `GlobalConnectionIpRestriction` — carry routing topology / CIDR data only, no
credentials, so they need no annotation.)

**EdgionStreamPlugins:** confirmed by review to carry no secrets (variants are IP/CIDR rules + a
plugin-reference name; runtime fields are `#[serde(skip)]`). No annotation needed.

The review also confirmed the EdgionPlugins serialization path actually reaches these config structs
(plugin chains serialize as `Option<Vec<Entry>>`; `Entry` uses `#[serde(flatten)] plugin: EdgionPlugin`;
`EdgionPlugin` is a plain tagged enum; only the runtime-compiled forms are `#[serde(skip)]`), so the
field annotations will fire.

### Admin choke point (structurally enforced)

The "scope is active only during synchronous serialization" invariant must be **compiler-enforced**,
not comment-enforced. Because the handlers are `async fn`, a future maintainer who adds an `.await`
while the guard is live would leak `redact_active = true` onto the worker thread and corrupt an
unrelated concurrent task's output.

Implementation: construct the `AdminRedactGuard` **inside the synchronous `list_client_resources`
function itself** (`api/mod.rs:104`) — the single shared sync frame that BOTH the list and the
single-get paths funnel through:

```rust
fn list_client_resources(client: &ConfigClient, kind: ResourceKind) -> Vec<serde_json::Value> {
    let _guard = AdminRedactGuard::new();   // RAII: scope confined to this sync fn
    match kind {                            // every serde_json::to_value below runs redacted
        ...
    }
}                                            // guard dropped here, before returning to async caller
```

This is the critical correctness point the review caught: `get_resource` →
`get_client_resource` (`api/mod.rs:118`) serializes the **entire** list via `list_client_resources` and
then filters the resulting `serde_json::Value`s by `metadata.name`/`namespace`. So placing the guard in
`list_client_resources` covers **both** `/configclient/{kind}/list` and `/configclient/{kind}?name=…`
with one site. Placing it only in a list-shaped wrapper (so the get path serialized outside the guard)
would re-leak every secret on the single-get path — the exact P1 this design closes.

The guard's lifetime is provably confined to this non-`async fn`: the whole chain
(`list_client_resources` → `get_dyn_cache(kind).list_json_dyn()` / `to_value`, and EdgionTls's path)
is fully synchronous — `list_owned` clones data out under a `parking_lot::RwLock` read and drops the
lock before `to_value`, with no `.await` anywhere — so the thread-local scope can never span an await
or leak to another task on the worker thread. The outer `Json(...)` re-serializes a plain
`serde_json::Value` and does not re-run the field adapters, so redaction must (and does) happen at the
`to_value` call inside the guarded frame.

### Implementation invariants (silent-failure guardrails)

A third review round confirmed the mechanism works against the current code, and isolated two ways a
future change could **silently** defeat redaction (compiles fine, leaks at runtime). Both must be
called out in code comments where relevant:

1. **Keep the serialization path single-threaded.** The guard is a thread-local; it only applies on the
   thread that runs `serde_json::to_value`. `list_json_dyn` / `list_owned`
   (`conf_sync/cache_client/cache.rs`) are currently sequential (`.into_iter().filter_map(to_value)`
   after dropping the `parking_lot` read lock). Introducing `rayon`/`par_iter`/`spawn_blocking` there
   would move serialization off the guarded thread and re-leak. Add a comment forbidding it.
2. **Never put an overriding serializer above a secret field.** The redaction relies on serde's derived
   `Serialize` reaching each annotated field. A hand-written `impl Serialize`, `#[serde(into=...)]`, or
   container-level `serialize_with`/`with` on any type ABOVE a secret field (e.g. `GatewayTLSConfig`,
   `EdgionPluginsSpec`, `SecretSlot`, `TlsCerts`, the EAB struct) would bypass the inner adapter.
   (Verified today: there are zero manual `impl Serialize` and zero `#[serde(into)]` in `src/types/`;
   `#[serde(flatten)]` is fine — it still drives the flattened struct's derived `Serialize`, so inner
   adapters fire.)

Both are "currently satisfied, must stay satisfied" properties — the cache stores typed `Arc<T>` and
serializes on demand (so field adapters fire at request time), not pre-built JSON.

### Data flow (one `#[derive(Serialize)]`, two binaries)

| Path | Binary | Scope guard | Output for secret fields |
|---|---|---|---|
| Controller → Gateway transport (`serialize_snapshot_json` / watch events) | Controller | absent (never constructed there) | real secret bytes (Gateway rebuilds them) |
| Admin `/configclient` list + get (guard inside `list_client_resources`) | Gateway | active | `"[redacted]"` |

### Error handling and failure direction

- The adapters cannot fail — they only choose an output form. The guard is infallible RAII.
- **The default (no scope) is fail-OPEN for admin exposure, not fail-closed.** This is deliberate and
  correct: making redaction the default would strip secrets from the controller's transport
  serialization and break the Gateway. The safety of admin redaction therefore rests on (a) the single
  choke point `render_resources_redacted`, and (b) `/configclient` being the only gateway-side
  serialization of these structs. Both are real and narrow; the spec states the trade-off plainly
  rather than overclaiming "fail-closed".

### Testing

- `redact.rs` unit tests: serialize a struct with an `Option<T>` secret field and a bare-`String`
  secret field — with the guard active → `"[redacted]"`; without → real values. Use the RAII guard
  itself for cleanup. Note: unlike `sensitive.rs` (whose `ModeGuard` needs a global `MODE_LOCK` mutex
  because `SENSITIVE_MODE` is a process-global `AtomicU8` shared across threads), the redact scope is a
  **thread-local** — cargo runs each test on its own thread and `Drop` (including on unwind) clears it,
  so **no global serializing mutex is required**. (The thread-local choice is also what makes the
  "no `.await` under guard" property safe: a global atomic held across `.await` would leak to all
  threads.)
- Transport regression test: assert that serializing a resource **without** a guard still emits the
  real secret (guards against accidentally inverting the default and breaking conf_sync).
- **Output-shape change + existing-test rewrite:** the old scrub set `Option` secrets to `None`, so
  `skip_serializing_if` made the field **vanish**; `redact_opt` instead emits a **present**
  `"[redacted]"` string for `Some`. The existing test `admin_serialization_strips_private_keys`
  (`api/mod.rs`, ~line 1590) calls the to-be-deleted `to_json_vec_edgion_tls` and asserts the secret
  fields are **absent** (`.get(...).is_none()`). Rewrite it to drive the new uniform path and assert
  the `"[redacted]"` shape. Any other test/consumer asserting *absence* of a `secret` key must be
  updated to expect a present redacted field.
- Per-kind redaction tests for the HIGH fields: Gateway listener `tls.secrets` (serving cert+key),
  KeyAuth `resolved_keys` (plaintext key in the map key), **EdgionAcme `external_account_binding`
  (inline `hmac_key`)**, EdgionTls (migrated from the deleted `scrub_edgion_tls_secrets` coverage), and
  one LinkSys provider (via `SecretSlot`) plus `RedisSentinel.password`.
- Integration: the conf-sync suites still pass (they assert only resource `count`, never secret bytes,
  so `"[redacted]"` is transparent).
- Acceptance checks:
  - Production mode (no `--integration-testing-mode`): `/api/v1/server-info` returns **404** (the route
    is genuinely absent from the production router, not 503); `/health` + `/ready` on probe port 28000
    return OK; `/configclient/gateway/list` returns 200.
  - **The secret-redaction assertion is primarily the deterministic per-kind unit tests** (construct a
    resource with the secret field populated, serialize under the guard, assert `"[redacted]"`). An
    end-to-end `curl /configclient/gateway/list` check only proves redaction if the live Gateway has
    `listeners[].tls.secrets` actually populated (native Gateway-API HTTPS path) — against an
    EdgionTls-only fixture it would pass *vacuously* (nothing to redact). So either point the e2e check
    at a fixture with a populated native-HTTPS listener, or rely on the unit tests for the guarantee.
  - `edgion-cli get gateway --target client` against a production gateway returns the resource with
    redacted secrets (still functional).
  - Testing mode: `/api/v1/server-info` reachable; conf-sync suite passes.
  - `cargo check --all-targets` / `cargo clippy --all-targets` clean (no new warnings).

## Explicitly out of scope

- **Full auth middleware (Bearer/mTLS/IP allowlist) for the gateway admin API.** Large cross-cutting
  feature needing a token-distribution mechanism; the controller's `unified_auth` does not extend to
  the gateway data plane. With mandatory redaction + loopback default, the admin port exposes no
  secrets, so this is deprioritized. If ever needed, follow the `allow_admin_ips` pattern in
  `tasks/todo/admin-api-https-and-access-control.md` Task 2.
- **Compile-time `#[cfg]` gating of the testing router.** Would break the `--integration-testing-mode`
  runtime-flag mechanism every conf-sync / integration script depends on. Not accepted.
- **`Sensitive<T>` newtype refactor.** Encoding "is secret" in the field type is the strongest
  anti-forgetting design, but it is a large invasive refactor across controller / gateway / resolvers /
  all read sites plus the transport unwrap. Recorded as a possible future direction; not done here.
  (The current `redact_opt` / `redact_required` adapters require each new secret field to carry the
  annotation — a residual forgetting risk this future refactor would eliminate.)
- **`#[derive(ScrubSecrets)]` proc-macro.** Equivalent declarativeness to the chosen serde-adapter
  approach but requires standing up a proc-macro crate and introduces a second "sensitive data"
  paradigm parallel to the existing access-log subsystem. Rejected in favor of reusing the existing
  idiom.
- **Free-form header/param values that an operator might paste a static token into**
  (e.g. `proxy_rewrite` / `ctx_set` header values, OpenID `authorization_params`). These are not
  designated secret fields — they are arbitrary user-authored config strings — so they are
  intentionally outside the field-level redaction model. No K8s-resolved or designated-secret material
  flows through them; an operator who hardcodes a bearer token into a header-injection value has placed
  plaintext in their own manifest. Recorded here so it is on the record, not a coverage gap.

## Wrap-up

- Per project convention (`feedback_delete_completed_task_files`), delete
  `tasks/todo/gateway-admin-api-harden.md` once the work lands; this spec + git history are the record.
- Add a decision note under `skills/04-review/architecture/` (sibling to the existing
  `gateway-admin-loopback-default.md`) describing: the mandatory admin-scope redaction mechanism, why
  it is independent of the operator-configurable access-log masking, and the Plan-B decision to keep
  `/configclient` mounted (so `edgion-cli -t client` keeps working) once redaction makes it safe.
