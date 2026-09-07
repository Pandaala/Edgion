---
name: string-len-bytes-vs-chars-contract
description: Use when a reviewer flags `check_string_len` / `OpCode::Add` / `JsonGet` byte-length guards as inconsistent with the `len()` builtin's character-count semantics, or proposes switching the runtime guard to `chars().count()`. Covers why the units are intentionally different and which suggested fixes are not accepted.
knowledge_type: intentional-design
---

# DSL `len()` (chars) vs `max_string_len` (bytes) — DIVERGENCE IS INTENTIONAL, CONTRACT NOW EXPLICIT

**Status**: Resolved (plugins-http-dsl-FB04, 2026-05-26) — `check_string_len` in `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs`, `RuntimeError::StringTooLong` in `edgion-dsl/src/error.rs`, and the `len` entry in `docs/{en,zh-CN}/.../dsl/api-reference.md`.
**Tests**: `tests::utility_functions::test_len_multibyte_returns_char_count_not_byte_count` (locks `len("héllo") == 5`, NOT 6).

## Conclusion

The DSL `len()` builtin returns **Unicode character count** (`s.chars().count()`) and the sandbox `max_string_len` limit — enforced by `check_string_len`, `OpCode::Add`, the regex builtins, and `JsonGet` — is measured in **UTF-8 bytes** (`s.len()`). The units differ by design: `len()` is a script-visible logical length, `max_string_len` is a memory-budget guard. The mismatch is now documented in the `len` entry of `docs/.../dsl/api-reference.md` and the `StringTooLong` error message names "bytes" on both sides ("`string too long (N bytes > L bytes)`").

Findings of the form "`check_string_len` uses bytes while `len()` uses chars" / "scripts cannot predict `StringTooLong` from `len()`" / "sandbox contract is inconsistent" describe a documented divergence, not a defect. Verify the `len` entry's "unit mismatch" note in `dsl/api-reference.md` (and the doc-comments on `check_string_len` + `RuntimeError::StringTooLong`) are present before re-reporting.

## Core Rationale

**1. Why `max_string_len` is in bytes**

`max_string_len` is part of the per-VM resource budget exposed on `VmLimits` (see `vm.rs:14-18`). Its purpose is **memory safety** for the sandbox: it caps the per-`Value::Str` heap footprint so a hostile or buggy script cannot blow the per-request memory envelope. The natural unit for that is the allocator-visible byte count, which is what `String::len()` / `&str::len()` already return — no extra scan, no extra cost, and the `max_steps × max_string_len` total-budget reasoning that informs the default 8192 is byte-denominated.

Switching `check_string_len` to `s.chars().count()` (the issue's original "preferred" suggestion) would:
- Re-define the sandbox memory budget in characters, silently raising the worst-case per-string heap footprint by up to 4× for valid UTF-8 (a 4-byte scalar like emoji consumes 4 bytes but counts as 1 char), and the `max_steps × max_string_len` aggregate budget by the same factor.
- Add an O(n) Unicode-scalar scan on every string-producing builtin call (currently a single field read).
- Not actually buy the script author's mental model parity unless every other byte-denominated guard (`Add`, `JsonGet`, regex result cap) is also flipped, multiplying both effects.

The memory-safety axis dominates the UX axis for a Low-severity, low-frequency footgun.

**2. Why `len()` is in characters**

`len()` is documented as Unicode character count and has been since the builtin was introduced. Changing it to bytes would be a **breaking semantic change** for any DSL script that does `len(token) < N` style validation — exactly the example in the API reference. The behavior matches `substr`'s character-indexed slicing, which uses `s.char_indices()`; both stay in the same character-count mental model. Adding a `byte_len()` builtin was considered and rejected: the explicit `len(s)` ≠ byte_len contract is loud, and a second variant would invite the wrong choice without explaining the trade-off.

**3. Why the fix is documentation + error-message naming, not unit alignment**

The footgun is "the script author cannot predict whether the next concat will throw `StringTooLong`". The realistic exposure is narrow:

- HTTP header values, path segments, and query strings — the primary `req.*` sources — are normally ASCII (RFC 9110 §5.5; non-ASCII bytes are %-encoded). The two unit counts coincide on the hot path.
- `StringTooLong` results in a 500 response, not a security boundary breach.
- Multi-byte content reaches the DSL mainly through synthetic test payloads or unusual integration cases.

So the cost-effective resolution is:
1. Make the `StringTooLong` message name its unit (`"... N bytes > L bytes"`).
2. Note the unit mismatch directly in the `len` API-reference entry with a worked example.
3. Lock the `len()` chars-not-bytes behavior with a multi-byte regression test (`test_len_multibyte_returns_char_count_not_byte_count`).
4. Cross-link the `check_string_len` and `RuntimeError::StringTooLong` doc-comments back to the API reference so future reviewers see the contract before opening another finding.

The script-author guard pattern `if len(s) < X { ... s + t ... }` is now unambiguous: `len()` is logical length, the concat limit is `max_string_len` bytes — the script author must either bound by byte budget (`if len(s) + len(t) < max_string_len/4 { ... }` as a conservative ASCII-and-multibyte-safe approximation) or accept that `StringTooLong` is a possible runtime fault and let it surface as a 500.

## Fix Suggestions Not Accepted

- **"Switch `check_string_len` (or `Add` / `JsonGet`) to `chars().count()` to match `len()`."** Re-defines the sandbox memory budget in characters, silently raising the worst-case per-string and aggregate footprint by up to 4×; trades a memory-safety guarantee for a UX cosmetic. Not accepted.
- **"Switch `len()` to byte count and document it / add `char_len()`."** Breaking semantic change for existing scripts; `len()` parity with `substr`'s character-indexed slicing is the language norm. Not accepted.
- **"Add a `byte_len()` companion builtin."** Adds API surface without retiring `len()`'s ambiguity, and invites the wrong choice; the doc note + explicit byte-named error message carries the contract more cheaply. Not accepted.
- **"Treat the divergence as a defect that needs runtime alignment."** It is an intentional UX/safety trade-off, recorded here and in the `len` API-reference entry. Re-reports framing this as "inconsistent sandbox contract" or "silent guard failure" should be closed by reference to this file.

## Re-evaluation Triggers

Revisit this decision if any of the following holds:

- A concrete production incident is traced to the byte/char gap (operator report, not theoretical exposure).
- A future feature ships that routinely carries multi-byte content through `req.*` builtins (e.g. JSON body inspection that materializes large UTF-8 strings into `Value::Str`), shifting the realistic exposure from "rare" to "common".
- The DSL adds an indexable rope-like string type or a streaming-string variant that decouples character count from heap footprint, at which point the byte-budget argument needs re-examination on a different cost model.

## Reference Cases

- `tasks/plugins/plugins-http-dsl-FB04-string-len-check-bytes-vs-chars-mismatch.md` — original finding (Low / BadLogic), closed by this entry.
- `docs/en/user-guide/http-route/filters/edgion-plugins/dsl/api-reference.md` (`### len`) and the zh-CN mirror — user-facing contract with the unit-mismatch note.
- `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs::check_string_len` and `edgion-dsl/src/error.rs::RuntimeError::StringTooLong` doc-comments — implementation-side pointers back to the API reference.
- `edgion-gateway/src/plugins/http/dsl/tests/utility_functions.rs::test_len_multibyte_returns_char_count_not_byte_count` — regression guard for `len()`'s character-count semantics on a 2-byte UTF-8 character.
