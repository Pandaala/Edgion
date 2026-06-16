# Gateway Admin API Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the gateway data-plane admin API `/configclient/{kind}` mandatorily redact every resolved/inline secret it serializes, and move the test-only `/api/v1/server-info` endpoint into the integration-testing-gated router.

**Architecture:** A new `edgion-resources/src/redact.rs` module provides a gateway-only thread-local RAII guard (`AdminRedactGuard`) and two generic serde `serialize_with` adapters (`redact_opt` for `Option<T>`, `redact_required` for bare fields). Secret-bearing fields on the synced resource structs carry the adapter; the guard is constructed inside the synchronous `list_client_resources` function so both the list and single-get admin paths serialize under it. Default (no guard) emits real values, so the controller's conf_sync transport serialization — in a separate process — is unaffected. This reuses the existing access-log "sensitive field" idiom (`edgion-resources/src/sensitive.rs`) but with independent, always-on ambient state.

**Tech Stack:** Rust, serde / serde_json, axum 0.8, k8s_openapi, tokio. Spec: `docs/superpowers/specs/2026-06-07-gateway-admin-api-harden-design.md`.

**Source-file language:** All code/comments in English (project rule).

**Commit policy:** This plan includes `git commit` steps. The project's CLAUDE.md says do not commit without explicit user instruction — the user has authorized committing for this plan's execution. Commit on the current branch (`feature-05-22`); do not push.

---

## File map

**Create:**
- `edgion-resources/src/redact.rs` — the redaction guard + two serde adapters + unit tests.

**Modify (wiring):**
- `edgion-resources/src/mod.rs` — register `pub mod redact;`.
- `src/core/gateway/api/mod.rs` — guard inside `list_client_resources`; delete EdgionTls imperative scrub; move `/api/v1/server-info` to the testing router; rewrite the EdgionTls test; add a router-shape test.

**Modify (field annotations — add `serialize_with`):**
- `edgion-resources/src/resources/common/secret_slot.rs`, `edgion-resources/src/resources/common/tls.rs`
- `edgion-resources/src/resources/edgion_tls.rs`
- `edgion-resources/src/resources/gateway.rs`
- `edgion-resources/src/resources/backend_tls_policy.rs`
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/{jwe_decrypt,jwt_auth,basic_auth,hmac_auth,openid_connect,key_auth,header_cert_auth,csrf}.rs`
- `edgion-resources/src/resources/link_sys/redis.rs`
- `edgion-resources/src/resources/edgion_acme.rs`
- `edgion-resources/src/resources/edgion_config_data/key_list.rs`

**Modify (docs/cleanup, final task):**
- doc comments in `api/mod.rs`, `conf_sync/conf_client/config_client.rs`, `conf_sync/cache_client/cache.rs`
- delete `tasks/todo/gateway-admin-api-harden.md`
- create `skills/04-review/architecture/gateway-admin-configclient-redaction.md`

---

## Task 1: Redaction module (`edgion-resources/src/redact.rs`)

**Files:**
- Create: `edgion-resources/src/redact.rs`
- Modify: `edgion-resources/src/mod.rs` (add `pub mod redact;`)

- [ ] **Step 1: Register the module**

In `edgion-resources/src/mod.rs`, add the line `pub mod redact;` in the alphabetical `pub mod` block (it sits between `pub mod public...`/`resources` — place it right after `pub mod resources;` on line 12, or anywhere in that contiguous list). Resulting neighbourhood:

```rust
pub mod resources;
pub mod redact;
pub mod schema;
pub mod sensitive;
```

- [ ] **Step 2: Write `edgion-resources/src/redact.rs` with the guard, adapters, and failing-first unit tests**

Create `edgion-resources/src/redact.rs` with this exact content:

```rust
//! Mandatory secret redaction for the gateway admin `/configclient` output.
//!
//! Unlike `crate::types::sensitive` (operator-configurable access-log masking),
//! this is an always-on security invariant with no config knob. A gateway admin
//! handler constructs an [`AdminRedactGuard`] around its synchronous serialization;
//! while the guard is alive on the current thread, the `redact_opt` / `redact_required`
//! serde adapters emit `"[redacted]"` instead of the real value.
//!
//! Default (no guard) emits the real value, so the controller's conf_sync transport
//! serialization — which runs in a different process and never constructs the guard —
//! is unaffected. See docs/superpowers/specs/2026-06-07-gateway-admin-api-harden-design.md.
//!
//! INVARIANT: the guard is a thread-local, so it only applies on the thread that runs
//! `serde_json::to_value`. Never move the admin serialization off that thread
//! (no rayon/`par_iter`/`spawn_blocking` in the `/configclient` serialize path), and
//! never add an overriding `impl Serialize` / `#[serde(into)]` on a type ABOVE a
//! secret field — either would silently bypass the field adapters.

use std::cell::Cell;

use serde::{Serialize, Serializer};

thread_local! {
    static REDACT_ACTIVE: Cell<bool> = const { Cell::new(false) };
}

/// True while an [`AdminRedactGuard`] is alive on the current thread.
#[inline]
pub fn redact_active() -> bool {
    REDACT_ACTIVE.with(|c| c.get())
}

/// RAII guard that turns on mandatory secret redaction for the current thread.
///
/// Construct it immediately before the synchronous serialization of admin output
/// (no `.await` may occur while it is held). Re-entrancy safe: it saves and restores
/// the previous flag value, and `Drop` runs on panic unwinding, so a panicking
/// handler cannot leave redaction stuck on for the next request on the same thread.
#[allow(clippy::new_without_default)] // `new()` has the side effect of activating the scope.
pub struct AdminRedactGuard {
    prev: bool,
}

impl AdminRedactGuard {
    pub fn new() -> Self {
        let prev = REDACT_ACTIVE.with(|c| c.replace(true));
        Self { prev }
    }
}

impl Drop for AdminRedactGuard {
    fn drop(&mut self) {
        REDACT_ACTIVE.with(|c| c.set(self.prev));
    }
}

/// serde adapter for `Option<T>` secret fields.
/// `#[serde(serialize_with = "crate::types::redact::redact_opt::serialize")]`
///
/// In an admin-redact scope, emits the scalar `"[redacted]"` (never recursing into
/// `T`, so the whole subtree collapses). Otherwise serializes the real value.
pub mod redact_opt {
    use super::{redact_active, Serialize, Serializer};

    pub fn serialize<T, S>(v: &Option<T>, s: S) -> Result<S::Ok, S::Error>
    where
        T: Serialize,
        S: Serializer,
    {
        if redact_active() {
            s.serialize_str("[redacted]")
        } else {
            v.serialize(s)
        }
    }
}

/// serde adapter for bare (non-`Option`) secret fields, e.g. `CsrfConfig.key: String`.
/// `#[serde(serialize_with = "crate::types::redact::redact_required::serialize")]`
pub mod redact_required {
    use super::{redact_active, Serialize, Serializer};

    pub fn serialize<T, S>(v: &T, s: S) -> Result<S::Ok, S::Error>
    where
        T: Serialize,
        S: Serializer,
    {
        if redact_active() {
            s.serialize_str("[redacted]")
        } else {
            v.serialize(s)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct Demo {
        #[serde(skip_serializing_if = "Option::is_none", serialize_with = "redact_opt::serialize")]
        opt_secret: Option<String>,
        #[serde(serialize_with = "redact_required::serialize")]
        bare_secret: String,
        public_field: String,
    }

    fn demo() -> Demo {
        Demo {
            opt_secret: Some("OPT-SECRET".to_string()),
            bare_secret: "BARE-SECRET".to_string(),
            public_field: "visible".to_string(),
        }
    }

    #[test]
    fn redacts_when_guard_active() {
        let json = {
            let _g = AdminRedactGuard::new();
            serde_json::to_value(demo()).unwrap()
        };
        assert_eq!(json["opt_secret"], "[redacted]");
        assert_eq!(json["bare_secret"], "[redacted]");
        assert_eq!(json["public_field"], "visible");
        let s = json.to_string();
        assert!(!s.contains("OPT-SECRET"), "opt secret leaked: {s}");
        assert!(!s.contains("BARE-SECRET"), "bare secret leaked: {s}");
    }

    #[test]
    fn emits_real_value_without_guard() {
        let json = serde_json::to_value(demo()).unwrap();
        assert_eq!(json["opt_secret"], "OPT-SECRET");
        assert_eq!(json["bare_secret"], "BARE-SECRET");
    }

    #[test]
    fn none_option_is_omitted_when_active() {
        let _g = AdminRedactGuard::new();
        let json = serde_json::to_value(Demo {
            opt_secret: None,
            bare_secret: "x".to_string(),
            public_field: "p".to_string(),
        })
        .unwrap();
        assert!(json.get("opt_secret").is_none(), "None field should be omitted");
    }

    #[test]
    fn guard_clears_on_drop() {
        assert!(!redact_active());
        {
            let _g = AdminRedactGuard::new();
            assert!(redact_active());
        }
        assert!(!redact_active());
    }
}
```

- [ ] **Step 3: Run the tests, expect PASS**

Run: `cargo test -p edgion --lib types::redact`
(If the crate name differs, use `cargo test --lib types::redact`.)
Expected: 4 tests pass (`redacts_when_guard_active`, `emits_real_value_without_guard`, `none_option_is_omitted_when_active`, `guard_clears_on_drop`).

> If the `const { Cell::new(false) }` form fails to compile on this toolchain, replace it with `Cell::new(false)` (plain init, used elsewhere in the codebase).

- [ ] **Step 4: Format + commit**

Run: `cargo fmt --all`

```bash
git add edgion-resources/src/redact.rs edgion-resources/src/mod.rs
git commit -m "feat(gateway): add admin-scope secret redaction module (types/redact.rs)"
```

---

## Task 2: Wire the guard into the admin path + migrate EdgionTls off imperative scrub

**Files:**
- Modify: `src/core/gateway/api/mod.rs` (`list_client_resources`, delete `scrub_edgion_tls_secrets` + `to_json_vec_edgion_tls`, the top-level `use`, doc comments, the EdgionTls test)
- Modify: `edgion-resources/src/resources/edgion_tls.rs` (annotate `spec.secret` + `client_auth.ca_secret`)

- [ ] **Step 1: Annotate the two EdgionTls secret fields**

In `edgion-resources/src/resources/edgion_tls.rs`, the `EdgionTlsSpec.secret` field currently reads (line ~170-172):

```rust
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[schemars(skip)]
    pub secret: Option<Secret>,
```

Change the `#[serde(...)]` line to:

```rust
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        serialize_with = "crate::types::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub secret: Option<Secret>,
```

And the `ClientAuthConfig.ca_secret` field (line ~77-79):

```rust
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[schemars(skip)]
    pub ca_secret: Option<Secret>,
```

→

```rust
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        serialize_with = "crate::types::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub ca_secret: Option<Secret>,
```

- [ ] **Step 2: Rewrite the EdgionTls admin-serialization test to expect `"[redacted]"` (failing first)**

In `src/core/gateway/api/mod.rs`, the test module `edgion_tls_scrub_tests` (around line 1531) currently has `admin_serialization_strips_private_keys` (line ~1590) which calls the soon-to-be-deleted `to_json_vec_edgion_tls` and asserts the secret fields are absent. Replace that whole `#[test] fn admin_serialization_strips_private_keys() { ... }` function with:

```rust
    /// The admin API must redact resolved private-key material, whether listed or
    /// fetched by name. Both `spec.secret` (leaf key) and `spec.client_auth.ca_secret`
    /// (mTLS CA key) must serialize as `"[redacted]"` through the guarded path.
    #[test]
    fn admin_serialization_redacts_private_keys() {
        let rt = tokio::runtime::Runtime::new().expect("create tokio runtime");
        let _enter = rt.enter();
        let client = ConfigClient::new("cid".to_string(), "cname".to_string());
        client.edgion_tls().reset(vec![edgion_tls_with_secrets()], 1);

        // List path: guard lives inside list_client_resources.
        let list = list_client_resources(&client, ResourceKind::EdgionTls);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["spec"]["secret"], "[redacted]", "leaf key not redacted");
        assert_eq!(
            list[0]["spec"]["clientAuth"]["caSecret"], "[redacted]",
            "CA key not redacted"
        );
        let serialized = serde_json::to_string(&list[0]).unwrap();
        assert!(!serialized.contains("tls.key"), "tls.key leaked: {serialized}");

        // Get-by-name path reuses list_client_resources, so the same guard applies.
        let got = get_client_resource(&client, ResourceKind::EdgionTls, Some("default"), "tls-cert")
            .expect("resource should be found");
        assert_eq!(got["spec"]["secret"], "[redacted]", "leaf key not redacted via get");
        assert_eq!(
            got["spec"]["clientAuth"]["caSecret"], "[redacted]",
            "CA key not redacted via get"
        );

        // Non-secret fields must survive so the endpoint stays useful.
        assert_eq!(got["spec"]["hosts"][0], "example.com");
    }
```

- [ ] **Step 3: Add `EdgionTls` to the test module imports**

In the same test module (`edgion_tls_scrub_tests`), the existing import is:

```rust
    use crate::types::resources::edgion_tls::{ClientAuthConfig, ClientAuthMode, EdgionTlsSpec};
```

Change it to include `EdgionTls` (which will no longer be imported at file top after Step 5):

```rust
    use crate::types::resources::edgion_tls::{ClientAuthConfig, ClientAuthMode, EdgionTls, EdgionTlsSpec};
```

- [ ] **Step 4: Add the guard inside `list_client_resources` and remove the EdgionTls arm**

In `src/core/gateway/api/mod.rs`, replace the whole `list_client_resources` function (lines ~97-112, including its doc comment) with:

```rust
/// List all resources of `kind` from ConfigClient as JSON, with secret fields
/// redacted for admin output.
///
/// An [`AdminRedactGuard`] is held for the entire (synchronous) serialization, so
/// every secret-bearing field annotated with the `redact_opt` / `redact_required`
/// serde adapters emits `"[redacted]"`. Both the list endpoint and the get-by-name
/// endpoint funnel through here, so both are covered by the single guard.
///
/// INVARIANT: keep this path synchronous and single-threaded — the guard is a
/// thread-local and would not apply if serialization moved to another thread.
/// Kinds not cached on the gateway (e.g. `ReferenceGrant`, `Secret`) resolve to
/// `None` and yield an empty list.
fn list_client_resources(client: &ConfigClient, kind: ResourceKind) -> Vec<serde_json::Value> {
    let _guard = crate::types::redact::AdminRedactGuard::new();
    client
        .get_dyn_cache(kind)
        .map(|cache| cache.list_json_dyn())
        .unwrap_or_default()
}
```

- [ ] **Step 5: Delete the imperative scrub functions and the now-test-only import**

In `src/core/gateway/api/mod.rs`:
- Delete `fn scrub_edgion_tls_secrets(...)` and its doc comment (lines ~138-153).
- Delete `fn to_json_vec_edgion_tls(...)` and its doc comment (lines ~155-164).
- Delete the top-level `use crate::types::resources::edgion_tls::EdgionTls;` (line 2). It is now referenced only from the test module, which imports it explicitly (Step 3).

Update the `get_client_resource` doc comment (line ~114-117) — it currently says "so the `EdgionTls` private-key scrubbing is inherited". Replace that doc comment with:

```rust
/// Get a single resource of `kind` by name (and namespace, unless cluster-scoped).
///
/// Reuses [`list_client_resources`], so it inherits the same admin-scope secret
/// redaction, then filters the JSON by `metadata.name` / `metadata.namespace`.
```

- [ ] **Step 6: Run the EdgionTls test, expect PASS**

Run: `cargo test --lib core::gateway::api::edgion_tls_scrub_tests`
Expected: `admin_serialization_redacts_private_keys` passes. (If the module path differs, run `cargo test --lib admin_serialization_redacts_private_keys`.)

- [ ] **Step 7: Verify compile (no unused-import warning)**

Run: `cargo check --all-targets 2>&1 | grep -i "unused\|edgion_tls" || echo "clean"`
Expected: `clean` (the moved import compiles; no `unused_imports`).

- [ ] **Step 8: Format + commit**

Run: `cargo fmt --all`

```bash
git add src/core/gateway/api/mod.rs edgion-resources/src/resources/edgion_tls.rs
git commit -m "feat(gateway): redact EdgionTls secrets via serde adapter + admin guard"
```

---

## Task 3: Annotate all remaining `Option<...>` secret fields with `redact_opt`

Every field below currently has a `#[serde(...)]` attribute containing `skip_serializing_if = "Option::is_none"` (some also `default`). **Recipe:** add `serialize_with = "crate::types::redact::redact_opt::serialize"` as another key inside that same `#[serde(...)]`, preserving the existing keys. Example transform:

```rust
// before
#[serde(default, skip_serializing_if = "Option::is_none")]
// after
#[serde(default, skip_serializing_if = "Option::is_none", serialize_with = "crate::types::redact::redact_opt::serialize")]
```

**Files / fields to annotate:**

- `edgion-resources/src/resources/common/secret_slot.rs` — `SecretSlot.secret` (line ~25 attr; field line 27)
- `edgion-resources/src/resources/common/tls.rs` — `TlsCerts.secret` (attr line ~89; field 91)
- `edgion-resources/src/resources/gateway.rs` — `GatewayTLSConfig.secrets` (attr ~189), `GatewayTLSConfig.resolved_frontend_ca_secrets` (attr ~198), `GatewayBackendTLS.resolved_client_certificate` (attr ~121)
- `edgion-resources/src/resources/backend_tls_policy.rs` — `resolved_ca_certificates` and `resolved_client_certificate` in BOTH the v1 spec (lines ~52, ~57) and the v1alpha3 spec (lines ~93, ~98)
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/jwe_decrypt.rs` — `resolved_credential` (field line ~143)
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/jwt_auth.rs` — `resolved_groups_by_iss` (field line ~202)
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/basic_auth.rs` — `resolved_users` (attr ~78; field 80)
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/hmac_auth.rs` — `resolved_credentials` (field line ~157)
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/openid_connect.rs` — `resolved_client_secret` (field ~207) AND `resolved_session_secret` (field ~210)
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/key_auth.rs` — `resolved_keys` (attr ~107; field 109)
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/header_cert_auth.rs` — `resolved_ca_secrets` (field ~122)
- `edgion-resources/src/resources/link_sys/redis.rs` — `RedisSentinel.password` (attr ~162; field 163)
- `edgion-resources/src/resources/edgion_acme.rs` — `EdgionAcmeSpec.external_account_binding` (attr ~280; field 281)

> For each: open the file, find the field, and add the `serialize_with` key to its existing `#[serde(...)]`. If a field's `#[serde(...)]` is written on one line, just append `, serialize_with = "crate::types::redact::redact_opt::serialize"` before the closing `)`. If multi-line, add it as another entry.

- [ ] **Step 1: Add a real-field redaction + transport-regression test in `secret_slot.rs`**

`SecretSlot` is the shared holder for all LinkSys auth secrets and has the same `Option<Secret>` shape as the Gateway/BackendTLSPolicy/EdgionTls fields, so it is the representative end-to-end check for the `redact_opt` annotation on a real type. Add this test module at the end of `edgion-resources/src/resources/common/secret_slot.rs` (if the file already has a `#[cfg(test)] mod tests`, add these two functions inside it and reuse its imports):

```rust
#[cfg(test)]
mod redact_tests {
    use super::*;
    use crate::types::redact::AdminRedactGuard;
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    fn slot_with_secret() -> SecretSlot {
        let mut data = BTreeMap::new();
        data.insert("password".to_string(), ByteString(b"SLOT-PRIVATE-KEY".to_vec()));
        SecretSlot {
            secret_ref: SecretObjectReference {
                group: None,
                kind: None,
                name: "s".to_string(),
                namespace: None,
            },
            secret: Some(Secret {
                data: Some(data),
                ..Default::default()
            }),
        }
    }

    #[test]
    fn secret_slot_redacted_under_guard() {
        let json = {
            let _g = AdminRedactGuard::new();
            serde_json::to_value(slot_with_secret()).unwrap()
        };
        assert_eq!(json["secret"], "[redacted]");
    }

    #[test]
    fn secret_slot_real_without_guard_for_transport() {
        // Transport regression: conf_sync (controller side, no guard) must emit the
        // real Secret object so the gateway can rebuild it.
        let json = serde_json::to_value(slot_with_secret()).unwrap();
        assert!(json["secret"].is_object(), "real Secret must serialize without guard");
        assert!(json["secret"]["data"].get("password").is_some());
    }
}
```

> If `SecretObjectReference` is not in scope via `use super::*`, add `use crate::types::resources::common::SecretObjectReference;` to the test module.

- [ ] **Step 2: Run the SecretSlot tests, expect PASS**

Run: `cargo test --lib resources::common::secret_slot`
Expected: `secret_slot_redacted_under_guard` and `secret_slot_real_without_guard_for_transport` pass.

- [ ] **Step 3: Compile-check all annotated files**

Run: `cargo check --all-targets`
Expected: builds with no errors (confirms all `serialize_with` paths resolve and types satisfy `T: Serialize`).

- [ ] **Step 4: Format + commit**

Run: `cargo fmt --all`

```bash
git add edgion-resources/src/resources/
git commit -m "feat(gateway): redact resolved secrets across Gateway/BTP/plugins/linksys/acme"
```

---

## Task 4: Annotate the bare-`String` secret fields with `redact_required`

These two fields are bare `String` (not `Option`), so they use `redact_required`. Neither currently has a `#[serde(...)]` attribute, so add a fresh one.

- [ ] **Step 1: Annotate `CsrfConfig.key` and add a test**

In `edgion-resources/src/resources/edgion_plugins/plugin_configs/csrf.rs`, the field (line 27) is:

```rust
    pub key: String,
```

Add the attribute directly above it:

```rust
    #[serde(serialize_with = "crate::types::redact::redact_required::serialize")]
    pub key: String,
```

Then add this test to the existing `#[cfg(test)] mod` in `csrf.rs` (the module that holds `test_serde_serialization`):

```rust
    #[test]
    fn csrf_key_redacted_under_guard() {
        let cfg = CsrfConfig {
            key: "SUPER-SECRET-KEY".to_string(),
            expires: default_expires(),
            name: default_name(),
            cookie_secure: default_cookie_secure(),
        };
        let redacted = {
            let _g = crate::types::redact::AdminRedactGuard::new();
            serde_json::to_value(&cfg).unwrap()
        };
        assert_eq!(redacted["key"], "[redacted]");

        let plain = serde_json::to_value(&cfg).unwrap();
        assert_eq!(plain["key"], "SUPER-SECRET-KEY");
    }
```

> `default_expires` / `default_name` / `default_cookie_secure` are the private default fns already defined in `csrf.rs`; they are in scope from the test module's `use super::*`.

- [ ] **Step 2: Annotate `MetaDataItem.key` and add a test**

In `edgion-resources/src/resources/edgion_config_data/key_list.rs`, the field (line 63) is:

```rust
    pub key: String,
```

Add the attribute directly above it:

```rust
    #[serde(serialize_with = "crate::types::redact::redact_required::serialize")]
    pub key: String,
```

Then add this test to the existing `#[cfg(test)] mod` in `key_list.rs`:

```rust
    #[test]
    fn metadata_item_key_redacted_under_guard() {
        let item = MetaDataItem {
            key: "API-KEY-123".to_string(),
            code: None,
            priority: None,
            id: None,
            behavior: None,
        };
        let redacted = {
            let _g = crate::types::redact::AdminRedactGuard::new();
            serde_json::to_value(&item).unwrap()
        };
        assert_eq!(redacted["key"], "[redacted]");

        let plain = serde_json::to_value(&item).unwrap();
        assert_eq!(plain["key"], "API-KEY-123");
    }
```

- [ ] **Step 3: Run both tests, expect PASS**

Run: `cargo test --lib "csrf_key_redacted_under_guard" && cargo test --lib "metadata_item_key_redacted_under_guard"`
Expected: both pass. Also confirm the pre-existing `csrf.rs` `test_serde_serialization` still passes (it serializes without a guard, so the key stays real):
Run: `cargo test --lib plugin_configs::csrf`
Expected: all csrf tests pass.

- [ ] **Step 4: Format + commit**

Run: `cargo fmt --all`

```bash
git add edgion-resources/src/resources/edgion_plugins/plugin_configs/csrf.rs edgion-resources/src/resources/edgion_config_data/key_list.rs
git commit -m "feat(gateway): redact CsrfConfig.key and KeyList MetaDataItem.key"
```

---

## Task 5: Part A — move `/api/v1/server-info` into the testing-gated router

**Files:**
- Modify: `src/core/gateway/api/mod.rs` (`create_admin_router`, `create_testing_router`, `router_shape_tests`)

- [ ] **Step 1: Add a failing router-shape test**

In `src/core/gateway/api/mod.rs`, in the `#[cfg(test)] mod router_shape_tests` block (around line 1730, which already has `status_of`, `test_client`, and `StatusCode` imports), add:

```rust
    #[tokio::test]
    async fn admin_router_drops_server_info_keeps_configclient() {
        // Production admin router (integration testing mode off in unit tests):
        // server-info is testing-only and must be absent; /configclient stays mounted.
        let r = create_admin_router(test_client());
        assert_eq!(
            status_of(r.clone(), "/api/v1/server-info").await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status_of(r, "/configclient/httproute/list").await,
            StatusCode::OK
        );
    }
```

- [ ] **Step 2: Run it, expect FAIL**

Run: `cargo test --lib router_shape_tests::admin_router_drops_server_info_keeps_configclient`
Expected: FAIL — `/api/v1/server-info` currently returns `200 OK` from `create_admin_router`, not `404`.

- [ ] **Step 3: Remove `server-info` from `create_admin_router`**

In `create_admin_router` (line ~193-200), delete the server-info route line so it becomes:

```rust
pub fn create_admin_router(config_client: Arc<ConfigClient>) -> Router {
    let mut router = Router::new()
        // Dynamic endpoints for all resource types (secrets redacted via the
        // AdminRedactGuard inside list_client_resources).
        .route("/configclient/{kind}", get(get_resource))
        .route("/configclient/{kind}/list", get(list_resources))
        .with_state(config_client.clone());

    // Add testing endpoints (incl. /api/v1/server-info) only in integration testing mode.
    if crate::core::common::config::is_integration_testing_mode() {
        router = router.merge(create_testing_router(config_client));
    }

    router
}
```

- [ ] **Step 4: Add `server-info` to `create_testing_router` (before `.with_state`)**

In `create_testing_router` (line ~500), add the route in the `State`-bearing group, before `.with_state(config_client)`:

```rust
fn create_testing_router(config_client: Arc<ConfigClient>) -> Router {
    Router::new()
        .route("/api/v1/testing/status", get(testing_status))
        .route("/api/v1/debug/store-stats", get(store_stats))
        // server-info needs State<Arc<ConfigClient>>, so it must precede .with_state.
        .route("/api/v1/server-info", get(get_server_info))
        .with_state(config_client)
        // ... remaining stateless routes unchanged ...
```

(Leave everything after `.with_state(config_client)` exactly as-is.)

- [ ] **Step 5: Run the router-shape test + the full router test module, expect PASS**

Run: `cargo test --lib router_shape_tests`
Expected: all pass, including `admin_router_drops_server_info_keeps_configclient` and the existing `admin_router_drops_health_and_ready_but_keeps_configclient`.

- [ ] **Step 6: Format + commit**

Run: `cargo fmt --all`

```bash
git add src/core/gateway/api/mod.rs
git commit -m "feat(gateway): gate /api/v1/server-info behind integration-testing mode"
```

---

## Task 6: Coverage verification, stale-doc cleanup, and wrap-up

**Files:**
- Modify: doc comments in `src/core/gateway/conf_sync/conf_client/config_client.rs` and `src/core/gateway/conf_sync/cache_client/cache.rs`
- Delete: `tasks/todo/gateway-admin-api-harden.md`
- Create: `skills/04-review/architecture/gateway-admin-configclient-redaction.md`

- [ ] **Step 1: Update the two stale doc comments that claim EdgionTls scrubs via the typed path**

In `src/core/gateway/conf_sync/conf_client/config_client.rs`, find the doc comment on the `edgion_tls()` accessor (it says something like "Retained as the only typed accessor because the admin API must scrub private-key material from `EdgionTls` ... which needs the concrete type"). Replace that justification with:

```rust
    /// Typed accessor for the `EdgionTls` cache.
    ///
    /// Admin-API secret redaction now happens uniformly at the serde layer
    /// (`types::redact`), so this accessor is no longer required for scrubbing; it
    /// is retained for typed test access.
```

In `src/core/gateway/conf_sync/cache_client/cache.rs`, find the `list_json_dyn` doc note that says "this performs no field redaction — callers that expose secret-bearing kinds (e.g. `EdgionTls`) must scrub through their typed path instead of this method." Replace it with:

```rust
    /// Serialize all cached resources of this kind to JSON.
    ///
    /// Secret redaction for admin output is applied by the field-level serde adapters
    /// (`types::redact`) when the caller holds an `AdminRedactGuard`; this method needs
    /// no special-casing. INVARIANT: keep this serialization synchronous and on the
    /// caller's thread (the guard is a thread-local).
```

- [ ] **Step 2: Verify every inventory field carries a redaction adapter (grep guard)**

This catches any missed annotation. Run:

```bash
cd /Volumes/ExtStore/ws4/Edgion
echo "Fields that hold Option<Secret>/Vec<Secret> but lack a redact adapter:"
grep -rEn "Option<Secret>|Option<Vec<Secret>>" edgion-resources/src/resources/ \
  | grep -v "redact_opt" || echo "  (none on the same line — verify manually below)"
echo "--- adapter usage count (expect ~17+ redact_opt + 2 redact_required) ---"
grep -rEc "redact_opt::serialize|redact_required::serialize" edgion-resources/src/resources/ | grep -v ':0' | sort
```

Manually confirm each `Option<Secret>` / `Option<Vec<Secret>>` field listed in the spec's inventory now has `serialize_with = "crate::types::redact::redact_opt::serialize"` on its `#[serde(...)]`, and that `CsrfConfig.key` + `MetaDataItem.key` have `redact_required`. (The grep prints the `#[serde(...)]` and field on separate lines, so eyeball the spec's "Field annotations — complete inventory" list against the matches.)

- [ ] **Step 3: Run the full local check suite**

Run, in order:
```bash
cargo fmt --all
cargo check --all-targets
cargo clippy --all-targets
cargo test --lib types::redact
cargo test --lib core::gateway::api
```
Expected: fmt clean; check + clippy with no new warnings; all redaction + router tests pass.

> Do not run the full `cargo test --all` or integration scripts here unless the user asks — those are heavier and run separately. The conf-sync integration suites (which assert only resource `count`) remain unaffected by redaction.

- [ ] **Step 4: Delete the completed task file**

Per project convention (completed tasks are removed; the spec + git history are the record):

```bash
git rm tasks/todo/gateway-admin-api-harden.md
```

(If `git rm` reports it is untracked, use `rm tasks/todo/gateway-admin-api-harden.md`.)

- [ ] **Step 5: Add the architecture decision note**

Create `skills/04-review/architecture/gateway-admin-configclient-redaction.md`:

```markdown
# Gateway admin `/configclient` secret redaction

**Decision:** The gateway admin API `/configclient/{kind}` endpoints stay always-mounted
(loopback by default), but every resolved/inline secret they serialize is mandatorily
redacted to `"[redacted]"`.

**Mechanism:** `edgion-resources/src/redact.rs` provides a gateway-only thread-local RAII
`AdminRedactGuard` and two generic serde `serialize_with` adapters (`redact_opt`,
`redact_required`). Secret-bearing fields on the synced resource structs carry the
adapter; the guard is constructed inside the synchronous `list_client_resources`
(`src/core/gateway/api/mod.rs`), covering both the list and get-by-name paths. Default
(no guard) emits real values, so the controller's conf_sync transport serialization
— a different process that never constructs the guard — is unaffected.

**Why independent from access-log masking:** access-log masking (`edgion-resources/src/sensitive.rs`,
`SENSITIVE_MODE`) is operator-configurable (Off/Replace/Wrap). Admin redaction is a
mandatory security invariant with no config knob; reusing `SENSITIVE_MODE` would let an
operator disable security redaction by turning off log masking. They share the serde-adapter
idiom but use separate ambient state.

**Plan B (keep `/configclient` mounted):** gating `/configclient` behind integration-testing
mode would 404 the shipped `edgion-cli get <kind> --target client` debug path. Since redaction
makes the endpoint leak-free, it stays mounted; only the test-only `/api/v1/server-info` moved
into the testing-gated router.

**Invariants (silent-failure guardrails):** keep the `/configclient` serialization path
single-threaded (the guard is a thread-local — no rayon/spawn_blocking), and never add an
overriding `impl Serialize` / `#[serde(into)]` on a type above a secret field.

Spec: `docs/superpowers/specs/2026-06-07-gateway-admin-api-harden-design.md`.
Sibling note: `gateway-admin-loopback-default.md`.
```

- [ ] **Step 6: Commit the wrap-up**

```bash
git add src/core/gateway/conf_sync/conf_client/config_client.rs \
        src/core/gateway/conf_sync/cache_client/cache.rs \
        skills/04-review/architecture/gateway-admin-configclient-redaction.md \
        tasks/todo/gateway-admin-api-harden.md
git commit -m "docs(gateway): update redaction doc comments, decision note; close task"
```

---

## Self-review checklist (completed during planning)

- **Spec coverage:** Part A (server-info move) → Task 5. Part B module → Task 1. EdgionTls migration + guard wiring → Task 2. All `Option` secret fields (shared holders, Gateway incl. listener `tls.secrets`, BackendTLSPolicy, EdgionPlugins resolved_*, LinkSys RedisSentinel, EdgionAcme EAB) → Task 3. Bare-`String` fields (Csrf, MetaDataItem) → Task 4. Stale-doc cleanup, coverage grep, decision note, task-file deletion → Task 6. Testing section (generic adapter, EdgionTls e2e, SecretSlot real-field, transport regression, per-bare-field, router 404) → Tasks 1–5; acceptance `cargo check/clippy/fmt` → Task 6.
- **Heavy-fixture note:** Per-kind tests for hard-to-construct structs that lack `Default` (e.g. `GatewayTLSConfig`, `KeyAuthConfig`, `EdgionAcmeSpec`) are intentionally NOT written as full fixtures. The generic adapter is proven definitively (Task 1) and on representative real annotated fields of each shape — `Option<Secret>` via `SecretSlot`, bare `String` via Csrf/MetaDataItem, end-to-end via EdgionTls — and Task 6 Step 2 greps to confirm every inventory field carries the adapter. This is the spec's stated test strategy ("primarily deterministic unit tests + annotation presence").
- **Type consistency:** `AdminRedactGuard::new()`, `redact_active()`, `redact_opt::serialize`, `redact_required::serialize` are used identically across all tasks.
```
