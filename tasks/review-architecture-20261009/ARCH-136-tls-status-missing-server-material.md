---
id: ARCH-136
status: candidate
kind: behavior-defect
priority: P2
partition: PART-411
analyst: /root/verify_ip_union
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# Preserve missing server material errors in EdgionTls status projection

EdgionTlsHandler::update_status in edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_tls.rs:402-426 reuses the parsed spec.secret and calls the canonical edgion_common::tls_validation::validate_cert. Its present-Secret branch filters SecretNotFound, CertNotFound and KeyNotFound before projecting the remaining errors into ResolvedRefsError::CertContentInvalid. The comment says the missing-material variants belong to the later secret-absent branch, but that branch is mutually exclusive with spec.secret.is_some() and cannot report missing fields within an existing Secret.

The production path admits this state. SecretHandler::parse normalizes stringData and stores the Secret without requiring server TLS data keys. SecretStore::get returns that object. EdgionTlsHandler::parse embeds any permitted present server Secret through ordinary_secret_material, which removes only the source-marker annotation and does not validate tls.crt or tls.key. A same-namespace core Secret reference with present Secret data lacking tls.crt therefore reaches update_status with spec.secret=Some.

The shared validator returns CertNotFound immediately when cert_pem fails. The Controller filter removes its sole error, and the existing resolved_refs_condition reports ResolvedRefs=True with All references resolved. A current, matching-host certificate with no private key similarly produces KeyNotFound without an unrelated certificate error, which the Controller suppresses. These are source-level witnesses, not executed reproductions. They do not require a denied reference, malformed parent identity or unresolved Secret lookup.

Gateway TlsStore::validated_entry uses the same validate_cert result without this filter. It rejects the new invalid material from ordinary matching; an eligible previous valid same-key generation may remain under the existing whole-generation last-known-good contract. The defect is a false successful Controller condition for the current unusable server material, not acceptance of a broken identity by the Gateway or a TLS authentication bypass.

Correct this local validator-to-status projection by retaining CertNotFound and KeyNotFound in the present-Secret branch and mapping their existing deterministic status_message through CertContentInvalid. SecretNotFound is unreachable in this branch under the validator's current early guard; retain at most that narrowly justified suppression, or remove the redundant filter entirely while keeping the existing absent/denied-reference branch. Reuse the current InvalidCertificate reason and typed projection. Do not duplicate certificate/key inspection, fetch the Secret again, add a validator framework or introduce a new condition type.

The concrete maintenance boundary is the canonical validator result. Both Controller diagnosis and Gateway admission already consume it; this extra classification discards meaningful failures and makes status depend on a second, incorrect definition of missing material. Preserving these existing errors fixes that divergence without changing certificate parsing or serving policy. Merely changing the explanatory comment would leave the false-success result.

Preserve InvalidKind and ReferenceGrant denial precedence, SecretNotFound for an absent source, dependency-before-lookup recovery, parsed-material ownership, per-parent controller identity, observed generation, deterministic messages, unchanged Accepted semantics and the absence of Programmed/Ready conditions. Do not include certificate or private-key bytes, parser input or secret values in diagnostics, and do not add metrics or Gateway tracing.

This correction is limited to the server certificate/private-key errors already emitted by validate_cert. The serving-chain-install regression guard deliberately retains hostname slots for a resource whose configured mTLS CA material fails later compilation; validate_mtls_config checks the CA reference shape rather than resolved CA content. Do not expand this finding into CA-content admission, collision precedence, chain validation, BoringSSL installation, same-key last-known-good policy or the pending typed resource-reference migration.

The current EdgionTls architecture page still describes certificate checks as validate-stage warnings and update_status as a SecretStore existence check. Actual source defers certificate-content diagnosis to update_status and reuses spec.secret. That prose is stale context, not evidence that missing server material should be hidden. Future implementation should correct the directly relevant description alongside this projection without changing unrelated TLS requirements.

Actual Controller TLS test bodies cover absent Secret, denied and permitted cross-namespace references, invalid kinds, valid matching material, key mismatch, expiry and native per-parent status. Common validator tests cover missing Secret/certificate/key, but the missing-key case uses fake certificate bytes and therefore does not establish the Controller's otherwise-valid-certificate witness. No current Controller test covers present Secret with only the filtered missing-material errors.

Future verification should drive real Secret parse/store to EdgionTls parse/status with absent data, absent tls.crt and a valid matching certificate lacking tls.key. Assert ResolvedRefs=False/InvalidCertificate, stable fixed messages and no secret bytes. Keep absent-source SecretNotFound, denied-reference RefNotPermitted, invalid-kind, valid recovery and existing mismatch/expiry behavior. Preserve Gateway rejection and same-key predecessor retention tests; no change to Gateway installation is required. No tests, builds or runtime reproduction were executed.
