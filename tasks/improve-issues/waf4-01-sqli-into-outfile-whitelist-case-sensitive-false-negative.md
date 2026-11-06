# waf4-01: `@detectSQLi` whitelists 3-token `INTO OUTFILE` / `INTO DUMPFILE` payloads when the keyword is not upper case (divergence from libinjection)

- **Category**: design-error
- **Severity**: P1
- **Scope**: local
- **Location**:
  - `edgion-waf/src/sqli/mod.rs:1274-1280` (`handle_three_token_whitelist`: `!self.tokens[1].val.starts_with(b"INTO")`)
  - `edgion-waf/src/sqli/mod.rs:485-521` (word merge copies the original-case bytes `into outfile` into the merged keyword token)
  - upstream reference: `libinjection-go@v0.3.2/sqli.go` `notWhitelist` case 3 (`toUpperCmp("INTO", s.tokenVec[1].val[:4])`, ASCII case-insensitive); C libinjection uses `cstrcasecmp("INTO", val, 4)`

## Summary
The native SQLi detector is pinned to libinjection-go v0.3.2. One whitelist branch does not match it.
For any 3-token fingerprint whose middle token is a keyword, the code treats the input as benign unless
that keyword starts with `INTO`. Upstream compares case-insensitively. Edgion compares the raw token bytes
case-sensitively. The word merge keeps the attacker's original case, so `into outfile` and `Into Dumpfile`
reach this check in lower or mixed case and are whitelisted. MySQL keywords are case-insensitive, so
`x' into outfile '/var/www/s.php` is the same attack as its upper-case form. Upstream detects it
(fingerprint `sks`); Edgion's `@detectSQLi` returns no match.

## Current behavior (evidence)
- Tokenization folds `into` + `outfile` through the 2-token word-merge rule. `sqli_data::lookup_word` upper-cases
  for the table lookup only. The merged bytes are copied verbatim into `token_vec[left].val`
  (mod.rs:511-514), so the stored value is `into outfile`.
- `is_not_whitelist` → `handle_three_token_whitelist` (mod.rs:1275-1280):
  `if self.tokens[1].len < 5 || !self.tokens[1].val.starts_with(b"INTO") { return false; }`. A lower-case
  `into ...` keyword therefore returns "not SQLi" even though the fingerprint is blacklisted.
- Differential run (built from the unmodified `src/sqli` sources against libinjection-go v0.3.2 compiled locally):

  | input | libinjection-go | Edgion |
  |---|---|---|
  | `x' INTO OUTFILE '/var/www/s.php` | true `sks` | true `sks` |
  | `x' into outfile '/var/www/s.php` | true `sks` | **false** |
  | `1 into outfile 'x'` | true `1ks` | **false** |
  | `'a' into dumpfile user0x1f` | true `skn` | **false** |

  Over 600k randomized SQL-ish inputs, every Edgion/Go disagreement (332 cases, 17 fingerprint shapes:
  `sks, skn, nks, nkn, 1ks, 1kn, sk1, 1k1, vks, nk1, 1kv, vkn, skv, nkv, vkv, vk1, ...`) was this branch.
  No other divergence appeared in the SQLi or XSS paths (a separate 400k-input XSS run had zero mismatches).
- The existing `libinjection_diff` corpus test (`tests/unit/lib_test/libinjection_diff.rs`) passes because
  its fixtures contain no 3-token lower-case `into` keyword.

## Impact
CRS 942100 (PL1, `@detectSQLi` on `ARGS|ARGS_NAMES|REQUEST_COOKIES|...`) uses only
`t:none,t:utf8toUnicode,t:urlDecodeUni,t:removeNulls`, with no lowercase transform, so attackers control
the case. Short MySQL file-write and webshell-drop payloads (`' into outfile '<path>`,
`' into dumpfile '<path>`, `1 into outfile 'x'`) pass the engine's primary SQLi detector. No CRS 4.25.1
data file or regex names `outfile`/`dumpfile`, so catching these depends on generic PL2+ regex rules.
Edgion claims upstream parity ("pinned to libinjection-go v0.3.2"), so operators reasonably expect the
same detection as Coraza/ModSecurity deployments.

## Suggested direction
Compare case-insensitively, exactly like upstream:
`self.tokens[1].len < 5 || !self.tokens[1].val[..4].eq_ignore_ascii_case(b"INTO")`.
Add the cases above to the pinned generated corpus. Also add a mixed-case mutation pass (random ASCII
case flips of each corpus input) to the differential generator. Every other case-handling site in the
port (`cstrcasecmp`, `lookup_word`) is already case-insensitive, so this one branch escaped review.

## Notes
- Checked `skills/04-review/waf/regression-guards/native-injection-operators-use-bounded-detection.md`.
  It records upstream pinning and generated-input comparisons, and explicitly says those do not prove
  complete equivalence. It records no accepted deviation for this branch.
- Not covered by waf-01..06 or waf3-01..05.

## Verification (independent)

- **Verdict**: CONFIRMED (P1)
- `edgion-waf/src/sqli/mod.rs:1278`: `!self.tokens[1].val.starts_with(b"INTO")` is a byte-exact, case-sensitive prefix test, while upstream libinjection compares the keyword case-insensitively; a lowercase `into outfile` keyword therefore hits the "treat as safe" branch.
- The original reviewer's differential harness (600k random inputs vs libinjection-go v0.3.2, scratchpad `waf4sqli`/`waf4go`) attributes all 332 disagreements to this single check; fix is a one-line `eq_ignore_ascii_case` plus a regression vector.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P1 unchanged.
- `edgion-waf/src/sqli/mod.rs:1278` is byte-identical: `if self.tokens[1].len < 5 || !self.tokens[1].val.starts_with(b"INTO")` (case-sensitive); `git diff feature-08-30..HEAD -- edgion-waf/src/sqli` is empty, so the word-merge (`:485-521`) still keeps original-case bytes.
- The differential harness dirs (`waf4sqli`/`waf4go`) are not present in any current scratchpad, so it was not re-run; with the detector sources unchanged, the prior 600k-input result still applies.
- `git diff feature-08-30..HEAD` shows no change under `edgion-gateway/src/plugins/http/waf/`, `edgion-gateway/src/ctx.rs`, `edgion-gateway/src/request_body/config.rs`, or `edgion-waf/src/` apart from a `serde_yaml`→`yaml_serde` dependency rename and a `fetch_update`→`try_update` rename in `json_inspection.rs:12`; none touch this finding's code paths.
