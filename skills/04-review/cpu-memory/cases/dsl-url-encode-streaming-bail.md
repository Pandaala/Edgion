---
name: dsl-url-encode-streaming-bail
description: Use when a finding flags that a DSL string-producing builtin (currently `url_encode`) "builds the full encoded result before the `max_string_len` check, allowing a transient allocation larger than the cap." The pattern was real for `url_encode` (fixed 2026-05-26 with an in-loop bail); the equivalent shape in other builtins should be evaluated case-by-case, not auto-applied.
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) and the sibling
> [dsl-step-budget-not-cpu-bound.md](./dsl-step-budget-not-cpu-bound.md).

# `url_encode` builds incrementally and bails on overflow (fixed)

**Original finding** (plugins-http-dsl F-B02, 2026-05-26): the previous body of
`BuiltinId::UrlEncode` in `src/core/gateway/plugins/http/dsl/lang/builtins.rs`
collected the full percent-encoded `String` via `s.chars().map(...).collect()`
and then called `check_string_len(&encoded, max_string_len)`. Because each
non-safe input byte expands to 3 encoded bytes (`%XX`), a maximally-sized
input could produce a transient allocation up to `3 × max_string_len` before
rejection (~24 KB at the default `max_string_len = 8192`). Bounded and
short-lived, but inconsistent with the rest of the codebase: `Split` pre-sizes
against `max_list_len` (`builtins.rs` ~line 510), and the other
string-producing builtins go through `checked_value_str`.

**Fixed shape**: the encoder now builds into a `String` initialized with
`with_capacity(s.len())` and checks `encoded.len() > limit` at the end of
every input-char iteration, returning `RuntimeError::StringTooLong` the moment
the running length crosses the cap. Maximum transient is bounded at
`max_string_len + a few trailing bytes from the partial char` instead of
`3 × max_string_len`, and legitimate inputs (those whose encoded form fits
under `limit`) behave byte-identically to the old code.

**Subtle but important — do NOT replace the in-loop bail with a coarse
pre-check.** A naïve "if `s.len() * 3 > limit` then error early" guard
(which is what the original issue file proposed) **rejects legitimate
inputs**: e.g. 4 000 bytes of ASCII alphanumerics encodes to 4 000 bytes
(safe chars unchanged), which is well under the default 8192 limit, but
`4000 * 3 = 12000 > 8192` — the coarse guard would erroneously fail it. The
in-loop bail avoids this false-positive class because it only fires once the
real running output has crossed the cap.

**Decision criteria during review**:

- Finding re-reports the "`url_encode` over-allocates before the size check"
  shape on `builtins.rs` `BuiltinId::UrlEncode` — **already fixed** (this
  file). Verify the current body uses `String::with_capacity(s.len())` + an
  in-loop `if encoded.len() > limit { return Err(StringTooLong) }`; if so,
  reject the re-report. If the implementation has reverted to
  `collect()`-then-`check_string_len`, that finding is a true regression of
  this fix.
- Finding proposes a coarse `s.len() * 3 > max_string_len` pre-guard (or any
  expansion-factor pre-guard that does not actually examine the encoded
  output) — **reject**: see the false-positive class above. Suggest the
  in-loop bail pattern instead.
- Finding flags the same encode-then-check shape on a *different* DSL
  builtin (e.g. `Base64Encode`, hex helpers, future encoders) — evaluate
  case-by-case. Conditions that make the in-loop bail worth porting:
  1. The encoded output can grow strictly larger than the input
     (`base64_encode` is 4/3, `url_encode` is up to 3×, hex is 2×).
  2. The input is already capped (any value from the VM stack is, since the
     producer paths all enforce `max_string_len`), so the worst-case
     transient is bounded but still a multiple of the cap.
  3. The pattern is cheap to apply (a small loop with an `if .. > limit`
     branch). If the builtin uses an external crate that returns a `String`
     (e.g. `base64::engine::Engine::encode`), porting the bail would require
     a custom encoder; weigh that cost against the transient size.
  Builtins that *cannot* exceed input length (e.g. `to_upper` /
  `to_lower` / `trim`) are not affected by this pattern — keep using the
  shared `checked_value_str` / `checked_value_opt_str` helpers.

**Not in scope**: this case is purely about per-call transient allocation
*inside one* builtin; it is not a defense against the four-cap product
described in `dsl-step-budget-not-cpu-bound.md`. The overall CPU / memory
bound is still set by the combination of `max_steps`, `max_call_count`,
`max_string_len`, and per-builtin output caps, not by this fix.
