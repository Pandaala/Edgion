# res4-01: Path normalization decodes `%5C` into a literal backslash but never treats `\` as a separator, so `..\` traversal survives normalization while the security contract claims `%5C` is covered

- **Category**: design-error
- **Severity**: P1 (security)
- **Scope**: local
- **Location**:
  - `edgion-resources/src/path_normalization/normalize.rs:254-263` (`is_decodable` whitelists `\`)
  - `edgion-resources/src/path_normalization/normalize.rs:170-188` (`do_normalize`: decode, then `merge_slashes` and `remove_dot_segments` that only know `/`)
  - `edgion-resources/src/path_normalization/normalize.rs:21-48` (`needs_normalization` has no backslash trigger)
  - `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs:971-992` (`apply_path_normalization_entry` - the only entry gate)
  - `skills/01-architecture/02-gateway/uri-normalization/SKILL.md` "Security guarantee scope" and `skills/04-review/uri-normalization/mandatory-design/normalization-contract.md` "Commitment Scope Summary" (claim `%5C` bypass is covered, "decoded then merged")

## Summary

The BASE algorithm percent-decodes `%5C` to a raw `\`, but its slash merge and RFC 3986 dot-segment
removal only recognise `/`. A request such as `/public/..%5Cadmin` therefore normalises to
`/public/..\admin`: it matches the `/public` route and any ACL keyed on `/admin` does not fire, and
the decoded raw backslash is then forwarded to the upstream by default (`forward_normalized_path: true`).
Any upstream that treats `\` as a path separator (IIS/ASP.NET, Windows file servers, several static
file servers and frameworks) resolves it to `/admin`. Edgion actively manufactures the dangerous byte:
without the decode the upstream would have received an opaque `%5C`. The review contract nevertheless
lists "`%2F` / `%5C` encoded slash bypass" as closed by default.

## Current behavior (evidence)

- `is_decodable` (`normalize.rs:258-262`) accepts `b'\\'`, so `percent_decode_whitelist` turns `%5C`/`%5c`
  into a raw backslash.
- `merge_slashes` (`normalize.rs:266-281`) and `remove_dot_segments` (`normalize.rs:297-361`) split on `/`
  only. `do_normalize` (`normalize.rs:184`) even gates dot removal on `path.split('/')` segments equal to
  `.`/`..`, so the segment `..\admin` is never treated as a dot segment.
- Raw (unencoded) `\` in the request target is also accepted end-to-end: httparse's URI map accepts
  `0x21..=0x7E`, and the `http` crate path table accepts `0x40..=0x5F` (which includes `0x5C`), so
  `/public/..\admin` sent verbatim is not rejected or rewritten either; `needs_normalization` has no
  backslash case.
- `apply_path_normalization_entry` (`pg_request_filter.rs:981-989`) only rejects escaped slashes when the
  operator opted into `reject_escaped_slashes`; the default stores the decoded form in
  `normalized_current_path`, which drives route matching, `KeyGet::Path` ACLs, and (by default) the
  forwarded target.
- Worked example (default config, routes `PathPrefix /` -> app and `PathPrefix /admin` -> same app with an
  IpRestriction/auth plugin): `GET /x/..%5Cadmin/users` -> normalised `/x/..\admin/users` -> matches `/`
  (no protection) -> upstream on Windows/IIS canonicalises to `/admin/users`.

## Industry comparison

- Envoy `normalize_path` uses Chromium's URL canonicaliser, which converts `\` to `/` before dot-segment
  resolution; `path_with_escaped_slashes_action: UNESCAPE_AND_FORWARD/REDIRECT` documents that `%5C`
  unescapes and is then normalised like `/`. Default (`KEEP_UNCHANGED`) does not decode `%5C` at all.
- NGINX on Windows treats `\` as a separator during URI normalisation; on Linux it leaves `%5C` encoded.
- Neither turns an encoded backslash into a raw one while leaving it outside dot-segment processing.

## Impact

Path-based ACLs (route split, `KeyGet::Path` conditions, IpRestriction/auth attached to a sub-prefix
route) can be bypassed against backslash-tolerant upstreams, which is exactly the "encoded slash bypass"
class the documentation says is closed by default. Operators are told only overlong UTF-8 / double
encoding / "non-compliant upstreams" are outside scope, so they have no reason to enable
`reject_escaped_slashes`.

## Suggested direction

Pick one canonical treatment of backslash and apply it to both raw `\` and decoded `%5C`:

1. Recommended (Envoy/Chromium parity): in `do_normalize`, translate `\` to `/` (raw and decoded) before
   `merge_slashes`/`remove_dot_segments`, and add `\` to the `needs_normalization` triggers. The normalised
   view and the forwarded path then agree with what backslash-tolerant upstreams will do.
2. Alternative: stop decoding `%5C` (drop it from `is_decodable`) and reject raw `\` at the entry gate
   (400). Simpler, but rejects some legitimate (rare) Windows-style clients.
3. Either way, correct the commitment table in the uri-normalization skill and review contract; today it
   claims a guarantee the code does not provide.

## Notes

- Checked `skills/04-review/uri-normalization/mandatory-design/normalization-contract.md` and
  `skills/01-architecture/02-gateway/uri-normalization/SKILL.md`: both list `%5C` as covered "decoded then
  merged"; neither mentions that the merged/dot-removed separator set excludes `\`. This is a knowledge
  error as well as a code gap (per AGENTS.md, the owning page should be corrected with the fix).
- `%2F` decoding is a separate, documented design choice (scenario 3 in the contract) and is not the
  subject of this finding.

## Verification (independent)

- **Verdict**: CONFIRMED (P1)
- `normalize.rs:258-262` `is_decodable` includes `b'\\'`, so `%5C` becomes a raw backslash; `merge_slashes` (`:266-281`) and the dot-segment gate in `do_normalize` (`:184`, `path.split('/')`) recognise only `/`, so `/x/..%5Cadmin` normalises to `/x/..\admin`.
- `needs_normalization` (`normalize.rs:21-48`) has no raw-backslash trigger, and http 1.5.0 `build_path_map` (`uri/path.rs:428`, range `0x40..=0x5F`) accepts raw `0x5C`, so raw `\` also passes unchanged.
- `pg_request_filter.rs:972-992` rejects escaped slashes only when `reject_escaped_slashes` is opted in; otherwise the decoded form becomes `normalized_current_path`, used for routing (`ctx.rs:4047-4049`), and `forward_normalized_path` defaults to `true` (`path_normalization/config.rs:73`).
- `skills/04-review/uri-normalization/mandatory-design/normalization-contract.md:225` claims "`%2F` / `%5C` encoded slash bypass: covered (decoded then merged)" - false for `%5C`; the knowledge page must be corrected with the fix.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P1
- `edgion-resources/src/path_normalization/`, `pg_request_filter.rs` and both uri-normalization skill pages are unchanged on this branch; `is_decodable` still admits a backslash and `remove_dot_segments`/`merge_slashes` still recognize only `/`.
