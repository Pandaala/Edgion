---
name: http-call-body-truncate-to-max-string-len
description: Use when a reviewer flags `http.call` returning a body up to `max_response_body_bytes` (1 MiB) that bypasses `max_string_len` (8 KiB) — or proposes switching the new body cap to a `RuntimeError::StringTooLong`. Covers why the cap is implemented as silent truncation and which suggested fixes are not accepted.
knowledge_type: intentional-design
---

# `http.call` body: silent truncation at `max_string_len` — RAISING IS NOT ACCEPTED

**Status**: Resolved (plugins-http-dsl-FB03, 2026-05-26) — `Vm::call_builtin` for `BuiltinId::HttpCall` in `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs` (after the `bytes_stream()` accumulation, before `result.insert("body", ...)`).
**Tests**: `tests::http_call_tests::test_http_call_body_truncated_to_max_string_len` (locks "server returns 16 bytes, `max_string_len=4` → `r["body"] == "AAAA"`, call still succeeds").

## Conclusion

After streaming-truncation at `runtime.max_response_body_bytes` (default 1 MiB), `http.call` further truncates the body to `self.limits.max_string_len` (default 8 KiB) **silently** before wrapping it in `Value::Str`. The effective body cap is `min(max_response_body_bytes, max_string_len)`. The truncation walks back to a UTF-8 character boundary so `String::truncate` cannot panic on multi-byte content.

Findings of the form "`http.call` body skips `check_string_len`" / "1 MB body can flow into `req.set_header` without size gating" / "sandbox string-length invariant violated by `r["body"]`" describe behavior that existed before this fix; the body cap restoration is now in place and the truncation pattern is intentional. Findings of the form "`http.call` body silently dropping data is wrong, it should raise `StringTooLong`" are also closed by this entry — see the rationale below.

## Core Rationale

**1. Why the body needs a `max_string_len` cap at all**

`http.call` writes the response body into the DSL value heap as a single `Value::Str`. Every other externally-controlled string builtin (`req.*` header / query / cookie reads, JSON string fields, base64 / hex / url decode, regex output, hash hex) enforces `max_string_len` before constructing the `Value::Str`. Without the same guard on `r["body"]`, a sandbox-invariant that holds everywhere else — "no DSL string value exceeds `max_string_len`" — would have a single carve-out, and an oversized body could then flow into `req.set_header` / `resp.set_header` / `ctx.set` / `log` (all of which `.into_string()` directly with no size check) up to the much larger `max_response_body_bytes` (default 128× `max_string_len`).

**2. Why silent truncation and not `RuntimeError::StringTooLong`**

The `http.call` API has a documented contract (`docs/en/.../dsl/api-reference.md`, "### http.call"): **"This function never raises a runtime error. Network failures, timeouts, and SSRF blocks all return `status = 0` with an empty body."** That contract is what lets a script author write `let r = http.call(...); if r["status"] == 200 { ... }` without a separate runtime-error policy branch on every call. Both other size caps in the same return value already follow the "silent cap, never raise" pattern:

- Body bytes capped at `max_response_body_bytes` — streaming loop discards the rest, no error.
- Headers map capped at `max_map_size` — `for` loop breaks on full, no error.

Raising `StringTooLong` specifically for the body-length axis would be a one-axis carve-out from the global `http.call` contract, forcing scripts that previously worked to either tune `max_string_len` upward (weakening the sandbox memory budget for *every* string in the VM) or write deny-policy fallbacks. The truncation path keeps the sandbox invariant intact while preserving the API contract.

**3. Why this differs from the rest of the module**

Other builtins use `check_string_len(...)?` and raise `StringTooLong`. The difference is the **call site's** error contract, not the value's role: `req.header()`, `json_get()`, `regex_find()`, etc. are documented as DSL-level operations that can raise; `http.call` is the one sandbox-edge function explicitly documented as non-raising. So `http.call` carries a different policy for the same value-construction step. The shared invariant being maintained is "the `Value::Str` finally inserted into the value heap is `≤ max_string_len`"; the mechanism by which different builtins enforce that (`?` vs `truncate`) is local to each builtin's documented error contract.

**4. Why truncation does not weaken security**

The memory peak is unchanged: the streaming loop already accumulates up to `max_response_body_bytes` bytes in `buf` before this point. Truncation only changes *what is kept* as a long-lived `Value::Str`, not *what is read* during the call. So downstream sinks (`req.set_header`, `ctx.set`, etc.) now receive a value of at most `max_string_len` bytes, matching what they would receive from any other builtin. Operators who legitimately need to see a larger body must raise `max_string_len`; the trade-off is then explicit at config time, not silent at call time.

## Fix Suggestions Not Accepted

- **"Use `check_string_len(&body_text, max_string_len)?` to raise `StringTooLong` on oversized body."** Breaks the documented `http.call` "never raises" contract; turns a `> max_string_len` upstream response into a deny / 500 response, forcing every script that touches `http.call` to handle a new error path. Not accepted; truncation preserves both the contract and the sandbox invariant.
- **"Always truncate at `max_response_body_bytes` only and document that the result may exceed `max_string_len`."** Leaves the sandbox-wide invariant "no `Value::Str` > `max_string_len`" with a single carve-out, which is the original FB-03 defect. Not accepted.
- **"Add a separate `httpMaxBodyStringLen` config to decouple the body cap from `max_string_len`."** Adds a config knob to solve a problem already solved by the existing `max_string_len` (the operator can already widen it). Not accepted as a Low-severity, narrow finding; the global `max_string_len` is the right knob.
- **"Truncate by byte index without UTF-8 boundary check (it's faster)."** `String::truncate` panics if the byte index is not a char boundary. Walking back from `max_string_len` to the nearest boundary is O(≤3) (UTF-8 scalars are ≤4 bytes) — negligible relative to the I/O cost of `http.call`. Not accepted.

## Re-evaluation Triggers

Revisit this decision if any of the following holds:

- The `http.call` "never raises a runtime error" contract is changed for a separate reason (e.g. a global error-policy redesign across builtins). At that point the truncation can be reconsidered as a raise.
- A future builtin returns externally-controlled byte buffers larger than `max_string_len` *that script authors expect to inspect in full* (e.g. file fetch, large JSON body materialization). At that point the right answer is likely the same — truncate at the value boundary — but the trade-off should be re-evaluated against any new error-handling pattern.
- A production incident is traced to silent truncation hiding upstream data that the operator needed; the fix would be to raise `max_string_len`, but if that turns into a recurring pattern, surface the truncation via a log or metric.

## Reference Cases

- `tasks/plugins/plugins-http-dsl-FB03-http-call-body-bypasses-max-string-len.md` — original finding (Low / BadLogic), closed by this entry.
- `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs` — the truncation site at the end of the `BuiltinId::HttpCall` arm.
- `docs/en/.../dsl/api-reference.md` and `docs/zh-CN/.../dsl/api-reference.md` — `### http.call` "Constraints" section, line that reads "body is truncated at `min(httpMaxResponseBodyBytes, max_string_len)`".
- `edgion-gateway/src/plugins/http/dsl/tests/http_call_tests.rs::test_http_call_body_truncated_to_max_string_len` — regression guard.
- See also `[[string-len-bytes-vs-chars-contract]]` for why `max_string_len` is byte-measured (the cap walked here is in UTF-8 bytes, which is why the boundary check is required).
