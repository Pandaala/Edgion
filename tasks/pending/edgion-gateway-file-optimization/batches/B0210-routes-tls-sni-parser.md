# B0210 — TLS SNI Parser

## Scope

- Pipeline state: verified; awaiting local commit.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/tls/sni_parser.rs`.
- Baseline: `aba8fd24`.
- Analyst: `b0206_tls_passthrough_analysis`, acting only as the B0210 analyst.
- Investigate bounded ClientHello record/fragment/name parsing and actual
  passthrough callers against the supported wire contract and existing tests.
- Implementation follows the exact two-path scope below. Integration files and user-owned paths remain protected.

## Analysis Outcome

- Decision: change; simple, medium-severity nested-length validation defect. Target blob `ccf79fed0244ec69a9a6e611587f6b05023b25ab` at `aba8fd24`; no tests run by analyst.
- Handshake u24 body length is ignored; extensions and SNI payload lengths are clamped to available bytes; ServerNameList length is skipped. A complete outer record can therefore yield a hostname outside its declared inner container. No authentication bypass, successful TLS handshake or out-of-bounds access is claimed.
- Function and owning knowledge already require invalid/internal truncation to return None. Enforce parent-bounded slices and complete structural checks, retaining only first-handshake bytes and rejecting malformed inner lengths without reading further records.
- Exact proposed scope: target and `skills/04-review/tls/regression-guards/sni-passthrough-clienthello-read.md`. No new library, protocol reassembly, hostname-character policy or duplicate-SNI policy. Any such expansion requires separate plan review.
- Keep outer record completeness tri-state unchanged, ASCII case normalization, valid TLS 1.2/1.3 parsing, unknown extension skipping, missing-SNI None, 16 KiB cap, absolute 15-second deadline and full-filled replay including trailing EarlyData.
- Retain all eleven existing tests; add short/oversized handshake, extension vector/header, SNI extension/list/entry parent-bound regressions, unknown extensions before/after SNI, extra handshake payload, trailing bytes and outer NeedMore coverage. Verify the complete structure rather than returning before later malformed boundaries are checked.
- Run parser and full TLS tests at promotion. D0118 tracks later malformed-input wire coverage; no frozen integration change.

## Promotion and Implementation

- Analyst revalidated at `cb9aaf4b`: parser, sole passthrough caller and original owning knowledge unchanged since `aba8fd24`. Nested length defect and exact failure semantics remain confirmed.
- Implementer: `b0210_implementation`; authorized the two files and local unit matrix above. Any wider protocol-policy or complex scope decision requires independent plan approval before expansion.

## Implementation and Verification

- Two-file implementation complete; parser blob `ccd2642a9f234d25a20a7c5ea44112db364d65dd`, corrected knowledge blob `9f83621fe52b8777bfd95ded66d65f71f0a09a9a`.
- Baseline Cargo reproduction confirmed an extension vector declared as 25 bytes with only 24 available still returned a hostname. Parent-bounded parsing now validates handshake, extensions and name-list/entry structure before allocating the selected name, preserving first selection and existing character policy.
- Original eleven tests retained; five matrix tests added. Standalone and Cargo parser execution each passed 16/16; fresh Cargo TLS passed 59/59, and broader routes passed 602/602.
- Independent reviewer: `b0210_final_review`; corrected LGTM, no remaining Must Fix or Should Fix at `cb9aaf4b`. Personally passed standalone and Cargo parser 16/16, docs and diff; no parser Clippy diagnostic. Workspace format/check/Clippy, Gateway build, documentation (1,264 files), SSA, metrics, Pingora source, Gateway tracing, English-only, diff and 3,120-file freeze passed. Clippy retains 983 Gateway lib-test warnings.
- Review distinguishes malformed nested lengths from a valid ClientHello fragmented across complete records: the latter remains unsupported by this first-record parser, not inherently malformed TLS. No complete supported single-record regression was found. Reviewer reproduced an oversized declaration; implementer confirmed actual 24/declared 25 in the original log and corrected the previously mistaken short-length attribution without source changes. Correction re-review passed.
- Commit: pending local commit; no push.
