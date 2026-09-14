---
name: wasm-secret-refs-status-derivation
description: Use when reviewing EdgionPlugins ResolvedRefs findings that a present-but-unusable or unauthorized Wasm fetch/OCI Secret still reports ResolvedRefs=True; covers the status derivation that now consumes usable-material outcomes.
status: fixed
finding_id: contract-02
closed: 2026-07-18
knowledge_type: regression-guards
---

# EdgionPlugins Wasm fetch/OCI Secret status derivation — FIXED

## Conclusion

`EdgionPluginsHandler::update_status` previously derived `ResolvedRefs` for Wasm
secret refs from Secret-object existence alone (`resolve_secret_refs` →
`secret_store.get(...).is_none()`), so a present-but-unusable Secret, a wrong
Secret type, or a cross-namespace ref with no `ReferenceGrant` (whose target
Secret exists) reported `ResolvedRefs=True` while the gateway was guaranteed to
fail closed on load. Fixed by a new read-only `resolve_wasm_refs` that re-runs
the resolver's usable-material checks for `authHeaderSecretRef` and
`oci.pullSecretRef` and maps each failure to the matching `ResolvedRefsError`.

Re-reports of the "false healthy ResolvedRefs for unusable/unauthorized Wasm
secret" form (for these two refs) are **not accepted** — verify
`resolve_wasm_refs` is wired into `update_status` before re-reporting.

## Core Rationale

**1. Status now consumes the resolver's actual outcome, not object existence**

`resolve_wasm_refs` (in `handlers/edgion_plugins.rs`) walks all four plugin
stages, and for each Wasm plugin re-derives the two refs read-only, mirroring
the resolver's short-circuit order (grant → existence → type → key):
- cross-namespace ref with no grant → `RefNotPermitted` (the resolver registers
  the dependency *before* the grant check, so the target Secret still looks
  present to the generic path — this is exactly the gap the fix closes);
- Secret object absent → **not** reported here (the generic `resolve_secret_refs`
  already emits `SecretNotFound`; deferring avoids a double message);
- `authHeaderSecretRef`: Secret present but the configured `key` is
  missing/empty/non-UTF-8 → `DecodeError` (key literal `"authHeaderSecretRef"`,
  the operator-configured data key name carried in `detail` because it is not
  `&'static` and cannot ride `MissingKeys(Vec<&'static str>)`);
- `oci.pullSecretRef`: wrong `Secret.type` → `InvalidSecretType`;
  `.dockerconfigjson` missing/empty/non-UTF-8 → `DecodeError`.

**2. Zero-drift with the resolver by sharing one check**

The usable-material predicate is the *same* `read_secret_utf8` the Wasm resolver
used at `parse` time (its visibility was widened from `pub(super)` to
`pub(in ...handlers)` for exactly this reason). Status and the resolver can never
disagree on "is this key usable".

**3. Gateway already fails closed — this is a status-accuracy fix only**

The three gateway consumers (`vm_manager.rs` fetch auth, `oci/mod.rs`
`RegistrySession::new`, `oci/auth.rs` `select_registry_auth`) already fail the
load when the resolved field is absent/invalid. The bug was purely a false
healthy K8s signal misleading operators / `kubectl wait` / automation; the data
plane was never at risk. No gateway change was made.

## Fix Suggestions Not Accepted

- "Have the generic `resolve_secret_refs` own all Wasm content checks" — it is
  shared across all plugin types and can classify only material kind,
  authorization, and object existence. Per-ref content still requires plugin
  knowledge, so the additive `resolve_wasm_refs` layer remains the correct
  place for key/type usability checks.
- "Report `SecretNotFound` from `resolve_wasm_refs` as well" — double-reports the
  common not-found path; the generic path already covers it.
- "Distinguish missing vs empty vs non-UTF-8 into separate reasons" — the
  resolver's `read_secret_utf8` collapses all three into one outcome; splitting
  them in status would claim a precision the resolver does not have.

## Follow-up Resolution (2026-07-22)

- `fetch.tls.clientCertificateRef` / `caCertificateRefs` grant-missing status:
  **DONE**. `collect_wasm_ref_errors` now classifies both refs; a shared
  `wasm_grant_denied` core backs `wasm_ref_grant_denied` (client-cert) and the
  new `classify_ca_certificate_refs` (each CA ref is a core Secret/ConfigMap;
  cross-namespace with no grant → `RefNotPermitted`). Grant-denial only, no
  PEM-content check (the CA/client-cert resolver validates none itself), so
  status stays faithful to the resolver. Covered by `tests::test_wasm_tls_*`.
- **Deep dockerconfigjson content** parity with the gateway's
  `select_registry_auth` (malformed JSON, bad base64 `auth`, non-`user:password`,
  or a registry host absent from `.auths`): **WON'T FIX** — do NOT re-report the
  false-healthy `ResolvedRefs=True` for a type-correct dockerconfig as a bug. A
  `kubectl create secret docker-registry` document is always well-formed, so
  host-mismatch is the only realistic case; the controller cannot depend on
  `edgion-gateway` to reuse that parser, so closing it would need a cross-crate
  extraction not worth the churn. The gateway fail-closes on the pull; status
  accuracy is the only cost. Rationale lives at the `resolve_wasm_refs` doc
  comment. Reconsider only if a real cluster shows repeated host-mismatch churn.

## Follow-up Resolution (2026-09-15)

- ConfigMap CA status kind preservation: **DONE**. ConfigMap material now uses
  `ConfigMap/ns/name` keys in the shared dependency manager, while Secret keys
  retain `ns/name`. `resolve_secret_refs` decodes that kind before the
  ReferenceGrant read and store lookup, so a valid ConfigMap-only grant no
  longer produces a false Secret denial or lookup. A missing ConfigMap reports
  `ConfigMapNotFound` rather than `SecretNotFound`.
- ConfigMap change/delete requeue uses the same typed key. Same-name Secret and
  ConfigMap dependencies can coexist without collision, while owner-level
  clear-before-add and delete cleanup remain unchanged.
- Verification includes a full Wasm parse-to-status positive case with only a
  ConfigMap grant, a missing-ConfigMap status case, and a ConfigMap event
  requeue test. Re-evaluate if another material kind is added to
  `caCertificateRefs` or dependency keys become structured values.

## Reference Cases

- `tasks/review-wasm-issues/contract-02-invalid-wasm-secrets-report-resolved.md` (2026-07-18)
- Source: `handlers/edgion_plugins.rs` (`resolve_wasm_refs`, `classify_auth_header_ref`,
  `classify_pull_secret_ref`, `classify_ca_certificate_refs`, `wasm_ref_grant_denied`,
  `wasm_grant_denied`);
  `handlers/plugins/resolve_credential/mod.rs` (`read_secret_utf8` visibility);
  `handlers/plugins/resolve_credential/wasm.rs` (the resolver being mirrored)
- Tests: `edgion_plugins.rs` `tests::test_wasm_*` (11 cases)
- Sibling Wasm findings: [architecture/wasm-module-cache-key-runtime-config.md](../../wasm/regression-guards/wasm-module-cache-key-runtime-config.md) (contract-01),
  [historical disk-purge interval clamp](../../cpu-memory/superseded/wasm-oci-purge-interval-zero-clamp.md) (contract-03, superseded)

## Resolution references

- `contract-02` — 2026-07-18.
