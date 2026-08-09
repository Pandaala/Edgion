# ExtProc — `mode_override` and `BUFFERED_PARTIAL`

| Key | Value |
|-----|-------|
| Created | 2026-08-04 |
| Status | todo — **ship in the same release as `01-response-body-pipeline.md`** |
| Where the change goes | Edgion only (`edgion-gateway/src/plugins/http/ext_proc/`, `edgion-resources` schema) — **no fork change** |
| Effort | S |
| Precedent | Envoy `ext_proc` (the only one of the three comparators with either) |

## Why it ships with 01, not after

`01` opens the response-body path. Without `mode_override`, an SSE response processed by an external processor pays **one processor round trip per token** — the capability is technically present and practically unusable. These two are what make streaming inspection affordable:

- **`mode_override`** — the processor can change the processing mode *for the rest of this request* based on what it has seen. The canonical use: inspect the first N bytes, then downgrade to `SKIP` for the remainder. Turns a per-chunk cost into a one-off.
- **`BUFFERED_PARTIAL`** — send the processor at most the first N bytes and then continue without waiting. The right mode for "classify this upload / sniff this response" policies that do not need the whole body and must not add per-chunk latency.

Both are pure plugin-side work: they are protocol-level modes in the `ext_proc` v3 wire format we already implement a subset of, and neither needs anything from the transport seam.

## Scope

- Accept both modes in the resource schema and validate them the same way the existing modes are validated (`is_supported_request` / the response allow-list). Keep the existing rejection of the modes we still do not implement.
- Honor `mode_override` in the request-body path that already exists, and in the response-body path `01` adds.
- `BUFFERED_PARTIAL` needs an explicit byte bound and a defined behavior at the bound: forward the remainder unprocessed, and say so in the plugin log rather than silently.

## What not to do

- Do **not** add `FULL_DUPLEX_STREAMED` alongside these. It is still WIP upstream, it forces trailer `SEND`, and it carries a separate mutation API — it is a different feature wearing the same family name.
- Do not let `mode_override` widen the mode. A processor downgrading itself (`STREAMED` → `SKIP`) is the point; a processor promoting itself mid-request would let it retroactively demand bytes the proxy already forwarded. Accept downgrades, reject upgrades, and log the rejection.

## Verification bar

- A processor that returns `mode_override: SKIP` after the first chunk must provably stop receiving chunks — assert the processor's received-message count, not just that the request succeeded.
- `BUFFERED_PARTIAL` at its bound must forward the remainder **byte-identically** — the existing streamed integration cases already assert "original chunk bytes must not leak through a mutation"; the mirror assertion here is "bytes past the bound must arrive unchanged".
- Both need the anti-vacuity rules from `tasks/todo/important_test/00-how-to-dispatch.md`: prove the processor was actually contacted before asserting anything about how it ended.
