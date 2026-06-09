---
name: conf-center-etcd-abstraction
description: Use when reviewing ConfCenter etcd abstraction boundary findings; documents why leaking etcd client config into to_client_config is an accepted tradeoff and will not be fixed.
---

# ConfCenter etcd Abstraction Boundary — Audited Design Tradeoff

**Audit date**: 2026-04-26
**Linked issue**: P1-09 (closed, not fixed)
**Conclusion**: **Known design tradeoff, will not fix.** When you see these "issues" in the next code review, read this document first before deciding to file a new issue.

## Code Involved

- `src/core/controller/conf_mgr/conf_center/etcd/config.rs:140-211`
  (`EtcdConfig::to_client_config`)
- `src/core/controller/conf_mgr/conf_center/etcd/center.rs:47-48`
  (`EtcdCenter::new` directly news `EtcdLinkClient`)
- `src/core/gateway/link_sys/providers/etcd/config_mapping.rs:38`
  (`build_connect_options` signature serves both the CRD flow and the file-config flow)

## Three Points That Look Like Issues (none will be fixed)

### 1. ConfCenter Synthesizes a Fake `Secret` / `SecretObjectReference` ("abstraction leak")

`config.rs:140-211` directly `use k8s_openapi::Secret` and `link_sys::TlsConfig`, and constructs a placeholder `secret_ref` (`name: "conf-center-yaml"`) + an embedded synthetic `Secret`.

**Why it is written this way**: to reuse the Gateway-side `build_connect_options` code path that handles TLS/auth, avoiding writing a second processing logic. The code comment already explicitly notes *"synthetic placeholder — secret_ref not used by the provider"*.

**Why it will not be fixed**:
- Only 2 callers (CRD flow + file-config flow); no third connection source has appeared
- Fixing would require rewriting the `build_connect_options` signature + designing `EtcdLinkClient` reconnect semantics (the reconnect path at `client.rs:126, 205` reuses the stored `EtcdClientConfig`; after changing the DTO you must think through how Secret updates are injected)
- This is precisely the just-fixed I4 silent regression (see "regression of the fix-change effort" in the `to_client_config` comment) — touching a just-fixed critical path for "elegance" is not worthwhile
- No runtime risk (not a memory/data leak, purely a design smell)

**When to reconsider fixing** (any of the trigger conditions reignites this discussion):
- A 3rd connection source appears (Vault / Consul / other KV)
- Someone wants to modify the field structure of `Secret` or `link_sys::TlsConfig`
- The abstraction leak causes a real bug

### 2. Construction Uses `anyhow::Result` vs Writer Uses `ConfWriterError` ("inconsistent error types")

**Not inconsistent — layered**:
- Construction-time errors (`EtcdCenter::new` / `to_client_config`): operator looks at context/stack, `anyhow` is appropriate
- Runtime writer operations (`CenterApi`): the caller needs to distinguish NotFound / Conflict, etc.; typed errors are appropriate

A common pattern in the Rust ecosystem. **Stop flagging this as an issue.**

### 3. `EtcdLinkClient` is Not Trait-ified ("hard to mock")

**YAGNI**:
- `EtcdLinkClient` has only one implementation in the entire repo
- The writer layer is already abstracted by `EtcdStorage` + `CenterApi` traits (this is the real abstraction boundary of ConfCenter)
- There is no real demand for "swap backend"; pre-extracting a trait only adds maintenance cost

**Stop flagging this as an issue**, unless you actually need to mock it (e.g., adding a fault-injection test).

## Hint for Future Reviewers

If you are auditing `conf_center/etcd/` and plan to file an "abstraction leak / inconsistent error types / interface not abstracted" type of issue — first confirm whether your reasoning goes beyond the 3 points already covered in this document. If the reasoning is the same, just close your draft issue.
