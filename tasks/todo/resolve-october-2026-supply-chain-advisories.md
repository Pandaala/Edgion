# Resolve October 2026 supply-chain advisories

## Status and scope

Pending independently planned security/dependency maintenance. Baseline commit: 6407efe23012ad6f53622f7ad53c33c662c1accb. Required `cargo deny check` reports 27 vulnerability advisories and one unmaintained dependency advisory. No new ignore is authorized. Preserve supported behavior and the Gateway BoringSSL-only boundary; affected rustls clients are ordinary outbound infrastructure, not Gateway rustls backend support.

## Verified evidence

The current uncommitted Gateway dependency pruning removes only two direct edges; all locked package versions, sources and checksums match the baseline. Independent diagnosis classified the failure as pre-existing, not a build/network/tool failure. Bans, licenses and sources pass. Complete local evidence: /private/tmp/edgion-b0001-guard-7.log.

| Locked dependency | RustSec advisory IDs | Required patched version on the relevant branch |
|---|---|---|
| pulldown-cmark 0.13.4 | RUSTSEC-2026-0414 | >=0.13.5 |
| rustls 0.23.45 | RUSTSEC-2026-0346 through 0349 | >=0.23.46 |
| rustls-webpki 0.103.15 | RUSTSEC-2026-0382 | >=0.103.16 |
| tokio 1.53.1 | RUSTSEC-2026-0386 | >=1.53.3 |
| unsafe-libyaml 0.2.11 | RUSTSEC-2024-0449 (unmaintained) | No patched version |
| wasmtime 49.0.2 | RUSTSEC-2026-0353 through 0364, 0378, 0379 | >=49.0.4 |
| webpki-root-certs / webpki-roots 1.0.9 | RUSTSEC-2026-0383 / 0381 | >=1.0.10 |
| zstd 0.13.3 | RUSTSEC-2026-0391 | >=0.14.3 |
| zstd-safe 7.2.4 | RUSTSEC-2026-0398, 0403 | >=8.2.0 |
| zstd-sys 2.0.16 | RUSTSEC-2026-0410 | >=2.1.2 |

Revalidate current primary RustSec records before implementation. Representative original sources: https://rustsec.org/advisories/RUSTSEC-2026-0391.html, https://rustsec.org/advisories/RUSTSEC-2026-0379.html, https://rustsec.org/advisories/RUSTSEC-2024-0449.html. The last advisory is maintenance status, not a demonstrated exploitable vulnerability.

## Required investigation and acceptance

1. Read root/nested AGENTS, knowledge index, graded rules, CI/supply-chain guidance and relevant dependency-owner contracts. Independently review a bounded plan and exact path list before mutation.
2. Trace active feature/version constraints and consumers. Patch updates may address several entries, but zstd 0.13 to 0.14 and zstd-safe 7 to 8 span Common, Pingora and Wasmtime constraints; preserve canonical Pingora source policy.
3. Investigate unsafe-libyaml through serde_yaml consumers in production crates, Kube and Pingora. Evaluate a properly reviewed parser/dependency replacement or evidence-backed risk disposition. Existing maintenance-mode guidance does not waive an active advisory. Do not silently bypass or bulk-ignore findings.
4. Preserve YAML decoding/schema/transport semantics, compression boundaries and frame handling, TLS trust/secret behavior, Wasm host isolation and resource limits, runtime cancellation and supported allocator combinations. Keep unsupported Gateway rustls work out of scope.
5. Require fresh cargo deny and audit results, required project checks, affected supported feature builds and meaningful regression/integration tests for changed dependency owners. Record residual issues accurately; do not report the supply-chain gate passing until it actually passes.

## Expanded raw-audit inventory during Stage 1

Raw cargo audit additionally exposed five baseline optional quinn-proto 0.11.16 advisories (RUSTSEC-2026-0338 through 0342); compatible 0.11.18 fixes all five and is independently approved as one additional precise lockfile root. Quinn is absent from every supported active feature graph because reqwest HTTP/3 is not enabled; this explains deny selection but is not an exemption.

Ten zstd-safe unsound warnings must remain tracked: RUSTSEC-2026-0395, 0396, 0397, 0399, 0400, 0401, 0402, 0404, 0405, 0406. All require >=8.2.0. cargo-deny 0.20.2 defaults unsound scope to workspace, excluding this transitive package; the four remaining deny errors are therefore not the full unresolved inventory.

Raw audit does not inherit deny.toml ignores. Existing explicit policies for hickory-proto RUSTSEC-2026-0119 and unmaintained derivative/proc-macro-error2/rustls-pemfile are unchanged, not newly accepted here. Keep raw audit JSON and strict `cargo audit --deny warnings --json` evidence alongside CI-comparable cargo deny. No new ignore is authorized.

## Partial repair and external prerequisite

Reviewed uncommitted Cargo.lock repair now substitutes 36 packages (SHA256 ce260fb93b56a345339cfc4b0f3cfc9897d64e8c58c8bea0f171bd27f190bfb1), removing 24 original deny advisories and all five Quinn records. Three Zstd vulnerability records, ten zstd-safe unsound warnings and unsafe-libyaml maintenance remain unresolved.

Canonical Pingora remote head 7708876aec0a3593b7b53970c6ee7899b8c34de8 still constrains header-serde to zstd 0.13.1 / safe 7.1.0 and core to serde_yaml 0.9; the header-serde path is nonoptional. Maintainers must adapt and publish fixed-major Zstd and a maintained YAML implementation in the existing fork. Current remote advancement alone cannot fix the gate. Local vendor/source overrides and new ignores are not authorized. After publication, separately review the unified Pingora update, Common Zstd, Wasmtime cache and workspace/Kube YAML migrations, then run all required gates.

## Further local remediation

Reviewed local phases now move Common List/Watch payload compression to zstd 0.14.3 / safe 8.2.0 and remove Wasmtime's unconfigured disk-cache feature while preserving every other default feature. Eleven exclusive cache dependencies are removed. Final lock SHA256: edec257534ba5369737089fe9c196e932af5ddc5625ed13bc5c2c4f33c36acf9. Both phases passed exact package/feature containment and independent targeted review, including codec interoperability and real Wasm transport. Workspace/allocator checks passed; final prepared default integration passed 279 groups with zero failures and 11 existing skips after a separately reviewed ResponseCache test synchronization repair.

Fresh security scans have the same advisory ID set and unchanged policy. Common and Wasmtime have exited the old Zstd graph; only Pingora core/header-serde directly retain old Zstd. Earlier next-step references to Common and Wasmtime are therefore completed local substeps, not remaining work. Canonical Pingora publication plus independently designed workspace/Kube YAML migration remain necessary before the full gate can pass.

User deferred further upstream dependency work while local code pruning continues. The remaining 14 records are open and scanner failures remain explicit; they are no longer a stop/commit gate for unrelated verified local batches. No new ignore is authorized and newly discovered findings require separate assessment.
