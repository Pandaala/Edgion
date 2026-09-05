---
name: local-response-body-is-clone-cheap-bytes
description: Use when reviewing findings about the local-response body representation — "ErrResponse should carry Bytes / an Arc<str> / a Cow", "why does PluginRunningResult not derive Serialize", "the Wasm local reply should sanitize its body to UTF-8", or "the DSL DenyWith body still allocates". Records the D0055 representation decision (2026-09-05).
status: fixed
finding_id: D0055
closed: 2026-09-05
---

# Local Response Bodies Are Clone-cheap Immutable `Bytes` — FIXED

## Conclusion

`PluginRunningResult::ErrResponse` carries `body: Option<bytes::Bytes>`. This is
the single canonical local-response body representation for every producer and
every request- and response-stage consumer. Fix suggestions of the form
"introduce an `Arc<str>` / `Cow<'static, str>` / a `StaticOrOwned` enum", "give
Mock (or any other plugin) its own body wrapper", "restore `Serialize` on
`PluginRunningResult`", or "make the Wasm local reply lossy-decode its body
again" are **not accepted**.

## Core Rationale

**1. The sink was already `Bytes`; only the enum field forced the copies.**

Both render points — the request-stage writer in `plugin_runtime.rs` and
`build_response_local_reply` in `session_adapter.rs` — end in
`bytes::Bytes`. With an owned `String` field, three costs were paid for
nothing: a per-request heap allocation for each of the ~40 fixed literal
bodies (`"...".to_string()`), a per-request deep copy of every
generation-owned configured body (Mock, FaultInjection's abort body,
RequestAccessPolicy's deny message), and a second deep copy at
`check_response_stage_terminate`'s `body.clone()`. `Bytes` removes all three
at once: `from_static` never allocates, a prepared field clones as a refcount
bump, `Bytes::from(String)` and `Bytes::from(Vec<u8>)` are moves, and the
writer needs no conversion at all.

**2. One type covers static and dynamic bodies, so no enum is needed.**

The obvious alternative — an enum separating shared-immutable from
dynamically-owned bodies — buys nothing here, because `Bytes` already
represents both without a branch at the consumer. An `Arc<str>` is strictly
worse: it keeps `&str` ergonomics but reintroduces a copy at the writer, since
turning it into `Bytes` means copying the bytes out.

**3. The serde/schemars derives were dead and were the only blocker.**

`PluginRunningResult` is a process-local plugin return value. It is defined in
`edgion-resources` only because both the schema layer and the Gateway name it;
it is never serialized, never deserialized, and never embedded in a
`JsonSchema` type or any CRD. Dropping `Serialize`/`Deserialize`/`JsonSchema`
is what allowed the `Bytes` field. `PartialEq`/`Eq` are retained — `Bytes`
implements both, so every existing `==` and `matches!` assertion is unaffected.

**4. Byte-exactness is a property, not a side effect.**

Because the body is raw bytes, `finish_local_response` in the Wasm plugin now
forwards the guest's buffer verbatim. The previous
`String::from_utf8_lossy(&local.body).into_owned()` both copied the buffer and
corrupted any binary body with U+FFFD replacement characters — a defect caused
solely by the `String` representation. Correspondingly, on a gRPC route the
body doubles as the `grpc-message` trailer, so the request-stage writer gates
it through `std::str::from_utf8` and falls back to the status-mapped default
message rather than emitting invalid header bytes.

## Fix Suggestions Not Accepted

- Introduce a `StaticOrOwned` / `Cow<'static, [u8]>` body enum — `Bytes`
  already represents both cases with no branch at any consumer.
- Migrate to `Option<Arc<str>>` — keeps `&str` ergonomics but forces a byte
  copy at the response writer, which is the copy this change exists to remove.
- Give an individual plugin its own body wrapper or a second body cache — that
  was the rejected B0089 shape; a plugin-local type cannot remove the copy at
  the shared enum boundary. Prepare the plugin's field as `Bytes` instead.
- Restore `Serialize`/`Deserialize`/`JsonSchema` on `PluginRunningResult` — no
  consumer exists, and the derives block the representation.
- Re-add the Wasm lossy UTF-8 decode, or sanitize a non-UTF-8 body on the HTTP
  path — the HTTP body is a byte stream; only the gRPC trailer needs UTF-8, and
  that is handled at the gRPC branch.
- Write `Some(Bytes::from("literal".to_string()))` for a constant body — that
  reintroduces the per-request allocation. Use `Bytes::from_static`.

## Known Residual (accepted, not a defect)

`DslErrorPolicy::DenyWith { body }` still allocates one `Bytes` per fail-close
(`body.clone().map(Bytes::from)`). `DslErrorPolicy` is the CRD configuration
type itself — it carries serde aliases and `JsonSchema` and is stored directly
as the DSL plugin's runtime policy — so its `body` cannot become `Bytes`.
The cost is identical to the pre-change behaviour, so this is not a
regression. Removing it requires a separate prepared runtime policy type for
the DSL plugin, which would ripple through ~20 test call sites; do not report
it as a defect of this change.

## Re-evaluation Triggers

Re-open this decision only if:

- `PluginRunningResult` acquires a genuine serialization consumer (an admin API
  surface, a conf_sync field, a CRD embedding) — then the body needs a serde
  adapter rather than a different representation.
- A local-response producer needs a body that is neither owned nor static (for
  example a zero-copy slice borrowed from the request body), which `Bytes` can
  express only by copying.
- The DSL plugin gains a prepared runtime policy type for other reasons, at
  which point the residual above becomes free to close.

## Reference Cases

- Deferred as `D0055` from batch `B0089` (Mock audit, 2026-09-01); canonical
  task `tasks/todo/use-clone-cheap-local-response-bodies.md`, closed 2026-09-05.
- `edgion-resources/src/filters.rs` — the enum and its representation contract.
- `edgion-gateway/src/plugins/runtime/session_adapter.rs` —
  `build_response_local_reply` plus the `local_reply_tests` covering absent vs
  empty bodies, non-UTF-8 preservation, static-buffer forwarding, and the
  bodyless-status path.
- `edgion-gateway/src/plugins/http/mock/plugin.rs` —
  `terminating_body_is_shared_across_requests_not_copied` pins the
  generation-owned sharing property.
- `skills/01-architecture/00-common/05-unification-points.md` §13 and
  `skills/01-architecture/02-gateway/12-edgion-plugin-dev.md` — the
  producer-facing rules.
