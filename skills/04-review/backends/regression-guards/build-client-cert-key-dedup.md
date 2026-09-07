---
name: build-client-cert-key-dedup
description: Read when reviewing build_client_cert_key and build_client_cert_key_from_secret sharing an identical PEM-parse block / both copies of the secret.data → X509::from_pem + PKey logic must be fixed twice (backends-F008).
knowledge_type: regression-guards
---

# backends-F008 — build_client_cert_key duplicate PEM-parse block

**Status / verdict**: fixed (backends-F008, 2026-05-24)

When reviewing findings of the form "`build_client_cert_key` and `build_client_cert_key_from_secret` share an identical PEM-parse block / both copies of the `secret.data` → `X509::from_pem` + `PKey::private_key_from_pem` → `CertKey::new` logic must be fixed twice / extract a `parse_cert_key_from_secret_data` helper (backends-F008)": fixed (backends-F008, 2026-05-24).

`build_client_cert_key` (`edgion-gateway/src/backends/peer_builder.rs`) now unwraps `policy.spec.resolved_client_certificate` (an `Option<Secret>`) and delegates to the existing `build_client_cert_key_from_secret(secret)` — both fields/params are the same `k8s_openapi::api::core::v1::Secret` type, so the prologue is the only real difference. The issue's suggested new `parse_cert_key_from_secret_data(&BTreeMap<..>)` helper is **not accepted**: delegating to the already-present `build_client_cert_key_from_secret` removes more duplication (including the `secret.data.as_ref()` line) without introducing a third function. Pure no-behavior-change DRY refactor (byte-for-byte identical output). Same class as backends-F007. Re-reports of the duplicate-block form are not accepted; verify `build_client_cert_key` delegates to `build_client_cert_key_from_secret` before re-reporting.
