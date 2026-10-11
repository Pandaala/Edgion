---
id: ARCH-151
status: candidate
kind: architecture-debt
priority: P3
partition: PART-412
analyst: /root/analyze_conversion
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# Reuse the existing Gateway frontend structural-validation owner

GatewayHandler's ProcessorHandler::validate implementation duplicates the complete Gateway frontend structural walk at gateway.rs:514–553, while validate_standard_frontend_tls at1628–1672 implements that same walk and parse calls it at652–653. Both check frontend.perPort cardinality, each port's range, duplicate ports, and default/per-port CA reference syntax through validate_frontend_ca_reference. The same immutable input therefore has two handwritten orchestration owners inside one handler.

The consumers have legitimate different purposes. validate emits ordered Gateway diagnostics, which ResourceProcessor merges before update_status; parse independently determines whether standard frontend projection must mark HTTPS listeners StrictReject. Those two phase checks should remain. The avoidable cost is maintaining two copies of the field traversal and constraint order: changing a per-port constraint or adding another frontend reference position requires updating both, or operator status and the security-sensitive projection decision can disagree. Current copies agree; no present handshake bypass, incorrect admission or measured performance regression is claimed.

Replace only the duplicated frontend block in validate with errors.extend(validate_standard_frontend_tls(g)) at its existing position. The helper already owns the full relevant result and is private, synchronous and IO-free. Preserve the early oversized-listener return, earlier Gateway diagnostics and later per-listener diagnostics. parse continues calling the same helper for its independent boolean decision. Do not introduce a cached verdict, transport field, new generic validation framework or a broad handler split. Removing the helper and sharing a smaller borrowed traversal is an alternative, but would unnecessarily rebuild a boundary already present in the file.

This is an ownership and future-change improvement, not a proposal to run the check only once or a cold-path speedup claim. The processor's validate-before-parse order and tests' direct parse calls justify retaining independent evaluation. Preserve exact diagnostic text/order/multiplicity, reference grammar, count limits, HTTPS-only projection, authoritative per-port selection including explicitly absent nested validation, raw private-field erasure and every StrictReject/fallback outcome. Keep immutable transport proofs, ordered CA material identities, Secret/ConfigMap dependencies, grant policy, serial workqueue processing, inline status IO, BoringSSL boundary and existing logging unchanged.

Actual existing tests frontend_runtime_validation_rejects_duplicate_range_and_nonempty_group and frontend_namespace_validation_accepts_internal_consecutive_hyphens exercise validate. standard_frontend_projection_is_https_only_authoritative_and_resolves_all_kinds, per_port_empty_tls_explicitly_disables_default_without_fallback and frontend_projects_sixteen_ca_references_in_fallback_mode exercise parse's projection contract. These bodies were read, not executed. Future verification should retain those subjects and compare ordered structural errors and malformed-projection outcomes for duplicate/range/count and CA identity cases, rather than adding tests that merely mirror the helper call.

Original schema-validation placement and strict frontend CA decisions were read. The accepted filesystem/etcd schema asymmetry does not require duplicate handler traversals, and independently defensive Gateway TLS compilation remains necessary. ARCH050 owns repeated per-entry plugin compilation within a schema-validation invocation; this finding owns two frontend policy implementations and deliberately retains both phase executions. No matching pending implementation was found. Static review only; no tests, builds, benchmarks, network activity or writes occurred.
