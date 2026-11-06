# waf-03: `t:htmlEntityDecode` requires a terminating `;` and case-sensitive names, diverging from ModSecurity/Coraza and browsers (XSS evasion)

- **Category**: design-error
- **Severity**: P2
- **Scope**: local
- **Location**: `edgion-waf/src/seclang/transform.rs:577-630` (`html_entity_decode`, `decode_entity`)

## Summary
Edgion decodes an entity only when it is terminated by `;` and only for lowercase names. ModSecurity v3
and Coraza (whose behavior CRS regexes are written and tested against) decode numeric entities without a
semicolon and match named entities case-insensitively; browsers do the same for numeric references and
legacy names. Payloads such as `javascript&#58alert(1)` or `&#x3c` therefore survive the transformation
unchanged and evade CRS rules that rely on it.

## Current behavior (evidence)
- `html_entity_decode` searches for `;` or `&` with `memchr2`; if no `;` is found, or `&` comes first, the
  `&` is copied literally (transform.rs:585-596). Then `decode_entity` parses `#x..`/`#..` strictly to the
  `;` — `&#58alert(1);` fails `parse_radix` and is copied verbatim.
- Named entities are matched byte-exactly (`b"lt"`, `b"amp"`, ...; transform.rs:617-628): `&LT;`, `&Lt;`
  are not decoded.
- The unit test pins this behavior: `(b"&amp", b"&amp")` in `html_entity_delimiters_preserve_decoding_and_output_limits`.

## Industry comparison
- ModSecurity v3 `HtmlEntityDecode::inplace`: for `&#`/`&#x` it consumes the longest digit run, decodes it,
  and "skip[s] over the semicolon if it's there"; named entities (`quot`, `amp`, `lt`, `gt`, `nbsp`) are
  compared with `strcasecmp` and the semicolon is also optional.
- Coraza `transformations/html_entity_decode.go` is a direct port with the same optional-semicolon rule.
- HTML5 tokenizers decode `&#58` / `&#x3a` without `;` (parse error, still decoded), and `&lt`, `&LT`,
  `&amp` legacy names without `;`, so `<a href="javascript&#58alert(1)">` executes.

## Impact
False negatives on CRS rules that use `t:htmlEntityDecode` (35 occurrences across 920/921/941/944 in the
pinned CRS 4.25.1), notably the 941 XSS regex rules (e.g. 941170's `javascript:` detection) at every
paranoia level. libinjection-based 941100 partially compensates, but the regex family is the defense-in-depth
layer CRS assumes; the CRS regression corpus is validated against the ModSecurity/Coraza semantics.

## Suggested direction
Port the reference algorithm: numeric references consume the maximal digit run with an optional `;`;
named references compare case-insensitively (keep the HTML5 extras already supported) with an optional
`;`; keep the existing output limits and borrowed-identity fast path. Add go-ftw/Coraza differential cases
for `&#58`, `&#x3c`, `&LT;`, `&lt` to the transform tests.

## Notes
Searched `skills/04-review/waf/` (only `semantic-transformation-reset-output.md` and the fused-JSON guard
mention this transformation, for ownership/borrowing, not semantics). No intentional-design page records
the strict-semicolon choice; the existing test documents current behavior, not a requirement. Not tracked
under `tasks/`.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P2 unchanged.
- `edgion-waf/src/seclang/transform.rs:577-607`: an entity is decoded only when `memchr2(';','&')` finds `;` first. `&#58alert` and `&#x3c` are copied literally.
- `transform.rs:609-631` `decode_entity` matches named entities byte-exactly (lowercase only). `&LT;` is not decoded.
- No `skills/04-review/waf` page records the strict-semicolon choice as intentional.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P2 unchanged.
- `edgion-waf/src/seclang/transform.rs` is unchanged; `html_entity_decode`/`decode_entity` at `:577-631` still require `;` and match named entities byte-exactly.
- No new `skills/04-review/waf` page accepts the strict-semicolon behavior.
- `git diff feature-08-30..HEAD` shows no change under `edgion-gateway/src/plugins/http/waf/`, `edgion-gateway/src/ctx.rs`, `edgion-gateway/src/request_body/config.rs`, or `edgion-waf/src/` apart from a `serde_yaml`→`yaml_serde` dependency rename and a `fetch_update`→`try_update` rename in `json_inspection.rs:12`; none touch this finding's code paths.
