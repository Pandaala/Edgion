# B0210 — TLS SNI Parser

## Scope

- Pipeline state: analysis-ready; third queued candidate, not the active mutation batch.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/tls/sni_parser.rs`.
- Baseline: `aba8fd24`.
- Analyst: `b0206_tls_passthrough_analysis`, acting only as the B0210 analyst.
- Investigate bounded ClientHello record/fragment/name parsing and actual
  passthrough callers against the supported wire contract and existing tests.
- No implementation authorized. Integration files and user-owned paths remain protected.

## Analysis Outcome

- Decision: change; simple, medium-severity nested-length validation defect. Target blob `ccf79fed0244ec69a9a6e611587f6b05023b25ab` at `aba8fd24`; no tests run by analyst.
- Handshake u24 body length is ignored; extensions and SNI payload lengths are clamped to available bytes; ServerNameList length is skipped. A complete outer record can therefore yield a hostname outside its declared inner container. No authentication bypass, successful TLS handshake or out-of-bounds access is claimed.
- Function and owning knowledge already require invalid/internal truncation to return None. Enforce parent-bounded slices and complete structural checks, retaining only first-handshake bytes and rejecting malformed inner lengths without reading further records.
- Exact proposed scope: target and `skills/04-review/tls/regression-guards/sni-passthrough-clienthello-read.md`. No new library, protocol reassembly, hostname-character policy or duplicate-SNI policy. Any such expansion requires separate plan review.
- Keep outer record completeness tri-state unchanged, ASCII case normalization, valid TLS 1.2/1.3 parsing, unknown extension skipping, missing-SNI None, 16 KiB cap, absolute 15-second deadline and full-filled replay including trailing EarlyData.
- Retain all eleven existing tests; add short/oversized handshake, extension vector/header, SNI extension/list/entry parent-bound regressions, unknown extensions before/after SNI, extra handshake payload, trailing bytes and outer NeedMore coverage. Verify the complete structure rather than returning before later malformed boundaries are checked.
- Run parser and full TLS tests at promotion. D0118 tracks later malformed-input wire coverage; no frozen integration change.
