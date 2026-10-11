---
id: ARCH-382
status: confirmed
kind: behavior-defect
priority: P3
partition: PART-503
analyst: /root/analyze_outbound
baseline: 93910657a31737c17726558f707ccce0e901fd11b669f0e948ef71b37a90f339
---

# Require the complete upstream path in PathNormalization forwarding assertions

Six request-client cases in `PathNormalizationForwardViewTestSuite` accept any echoed path beginning with the expected path. `tc_a_default_normalized`, `tc_b_annotation_raw_true` and `tc_c_annotation_norm_false` describe this as accepting a query string attached to the path; the rewrite, multi-step and invalid-annotation cases use the same prefix acceptance. However, the actual selected backend's `catch_all_handler` emits `req.uri().path()` as `Path:` and emits the query separately as `Query:`. Arbitrary path suffixes therefore satisfy a probe intended to verify the complete forwarded path.

A concrete false-PASS witness for `tc_e_multi_step_normalization` is a successful completed backend echo containing `Path: /api/foo/bar/extra` instead of `/api/foo/bar`. Its `p == expected || p.starts_with(expected)` guard returns PASS. Both fixture trees select the `/api` PathPrefix route and the test-http wildcard echo backend, so this different upstream path can still receive a normal 200 response. Similarly, `/api/data-wrong` satisfies the default, normalized-annotation and invalid-annotation assertions; `/rewritten/data-wrong` satisfies the rewrite-precedence assertion. These are source-derived wrong-output witnesses, not observed forwarding failures.

The newer `absolute_form_consumers` case already compares parsed path and query exactly for four host variants. This is useful counterevidence: complete output assertions are feasible without a new server endpoint or production observability. It does not cover the multi-step input or invalid-annotation host, and it does not justify weakening the separate origin-form cases. Production `finalize_request_uri` owns the exact selected path and preserved query; its actual unit tests check complete URI values and the finalization latch. The normalization review contract also identifies path/query preservation as the relevant consumer invariant.

Replace the permissive path-prefix success branches with equality against each case's expected complete path. Use the existing separate-query parser if query preservation is included in that case's stated contract. If an alternate echo format genuinely appends a query to `Path:`, split only at its explicit `?` delimiter before comparing the entire path; arbitrary suffixes must still fail. A small private assertion helper is optional, not a required shared framework. Keep missing-path, transport and body-read failures as failures.

Preserve all case identities, requests, fixtures, expected statuses, raw-versus-normalized and rewrite precedence, optional canonical access-log checks, listener resolution and wrapper deadlines. Do not change normalization, URLRewrite, matching or request/response body ownership in production. Global restart-dependent scenarios and the deliberate mode-agnostic PreserveParam suite remain outside this correction. Verification should challenge each affected assertion with exact success, an appended path segment, an appended non-separator suffix and a missing path, while retaining the real backend echo contract. No execution or measured performance result is claimed.
