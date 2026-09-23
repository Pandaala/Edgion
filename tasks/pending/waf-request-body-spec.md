# WAF Request Body Inspection and Early Prefix Cache

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-23 |
| Status | in-progress |
| Type | feature |
| Priority | P0 |
| Design | Consolidated user-approved requirements; integration verification pending |
| Tracking task | [Request verdict before forwarding](../todo/waf-request-verdict-before-forwarding.md) |

## Summary

The default WAF body-inspection path caches a bounded prefix **before upstream
dispatch**, evaluates that prefix, and then forwards the entire allowed request.
Partial caching must preserve the exact payload, byte order, and completion of
the remaining stream. Only explicit `full` inspection requires complete capture
before the body verdict. Reuse Edgion's request-scoped `BodyStorage` and Pingora's
registered request-body buffer; introduce no second body-cache implementation.

This spec is the current design authority for the linked task. It supersedes
earlier fixed-window and prefix spill-threshold proposals.

## Configuration

```yaml
spec:
  waf:
    requestBody:
      inspection: prefix
      prefixSize: 64KiB
```

| Field | Default | Contract |
|-------|---------|----------|
| `inspection` | `prefix` | `prefix` or `full` |
| `prefixSize` | `64KiB` | Size string resolving to 1 byte through 1 MiB, inclusive; only valid with `prefix` |

Sizes use the shared 1024-based parser: examples include `16KiB`, `100k`, and
`1m`. There are no fixed size tiers. Reject invalid sizes, zero, values above
1 MiB, and explicit `prefixSize` combined with `full`. Resolve byte counts while
loading configuration; do not parse sizes on the request path. Controller
validation and Gateway construction enforce the same rules.

Complete inspection is explicit:

```yaml
spec:
  waf:
    requestBody:
      inspection: full
```

## Default request flow

1. Select the effective WAF generation and execute request-header rules. A header
   denial can return immediately without reading the body.
2. When body inspection is required, obtain the prefix before backend selection,
   connection, or upstream request-header transmission. Wait for the configured
   window or actual body EOF, whichever arrives first. WAF configurations that
   do not consume the body need not read it solely to populate a cache.
3. Seal the prefix storage and evaluate the supported body rules. Sealing a full
   inspection window is not transport EOF and does not publish a complete body.
4. On denial, return the configured response: the backend receives no request
   headers or body. On admission, begin ordinary upstream forwarding.
5. Drain the cached prefix, then any bytes already read beyond the window, then
   read and forward the remaining downstream body with normal backpressure.
   Release retained data when its delivery/ownership contract permits.

For a body shorter than the window, the prefix contains the entire body because
actual EOF was reached. Larger bodies do not wait for full capture. Passing the
prefix check means only that no applicable rule blocked the inspected window;
bytes outside that window receive no further WAF body inspection in this mode.

## Cache ownership and memory policy

- For a new WAF-only prefix capture, `BodyStorage`'s memory and total capacities
  both equal the inspection window. All supported windows, including 1 MiB,
  stay in memory; the complete-body spill threshold does not apply to this lane.
- If a smaller physical request-body cap applies, capture needs at most that
  cap plus one byte to prove overflow. It must reject overflow rather than admit
  a silently shortened inspection window.
- Transport reads can cross the window boundary. Keep the overrun separate from
  inspection storage, under an explicit transport-chunk bound, for later delivery.
  The prefix cache limit is not a claim that total request memory equals that
  limit: transport buffers, overrun, and WAF working memory also exist.
- Reuse a complete body cache already produced by another consumer, reading only
  its current logical prefix. It may already reside on disk. Freeze its logical
  representation after inspection so forwarding cannot send a different body.
- Never expose a partial prefix as a complete `RequestBodyState`. A later
  complete-body consumer must fail closed if only a partial capture is available;
  it must not reinterpret that prefix as the full body or create another cache.
- Incompatible mutations or streaming processors fail closed. Read-only
  observers continue through the canonical forwarding path.

## Payload-preservation contract

For a new prefix capture, let `B` be the logical request payload before WAF inspection, `P` the retained
prefix, `O` any transport overrun, and `R` the unread remainder. For an admitted
request that completes without a transport or policy error:

```text
B = P || O || R
upstream payload = P || O || R
```

Every payload byte must be delivered exactly once and in order. In particular:

- Do not drop bytes when a chunk crosses the inspection boundary.
- Do not forward the captured prefix a second time from another replay source.
- Do not end the upstream body when the prefix cache ends; join the live source.
- Do not forward any body bytes while the prefix decision is still pending.
- Preserve HTTP message framing through the canonical H1/H2 paths.
  Chunk/frame boundaries may change; payload content and message completion must
  remain correct.
- Count physical bytes when read from transport, never again during replay.
- A failed or cancelled capture is not an admissible replay source. Fail closed
  and preserve existing connection-reuse and cleanup rules.
- Native prefix captures are not retryable, including short bodies that reached
  EOF. A pre-existing complete cache keeps its established retry policy.
- Handle `Expect: 100-continue` locally when early capture requires it; do not
  wait for an upstream request that admission intentionally has not started.

When reusing an existing complete cache, all payload bytes come from that same
frozen logical cache, including the part after the inspection window. There is
no unread downstream remainder to consume in this case.

Trailer behavior requires separate verification: the existing registered-buffer
contract does not capture or replay request trailers. This spec's payload
identity guarantee does not establish trailer preservation when early capture
consumes them. Record that limitation in integration results; do not describe
the current cache as preserving every aspect of the HTTP message. Any extension
to retain captured trailers requires an explicit transport design and review.

## Full inspection and independent limits

`full` uses the shared complete `BodyStorage` path, waiting for actual EOF before
the body verdict and upstream dispatch. The existing memory threshold, disk
spill, storage timeout, and physical body-size policy apply. Full inspection is
still bounded by gateway and WAF execution limits; it is not an unlimited read.

Prefix inspection uses raw HTTP payload bytes after transfer framing is removed;
it does not decompress `Content-Encoding`. Initially it accepts only the proven
positive raw-body rule subset. Rules requiring absence, structured complete
documents, unsupported chains/control flow, or later-phase whole-body values
must use `full` and remain subject to the engine's supported-rule validation.

The physical request-body cap is independent of `prefixSize`. An overflow found
after prefix admission can occur after upstream exposure. `detectionOnly`
suppresses rule interventions, but does not suppress transport, storage, physical
limit, or execution-budget failures.

## Acceptance criteria

| Area | Required evidence |
|------|-------------------|
| Defaults and configuration | Omitted policy selects 64 KiB prefix; arbitrary valid sizes up to 1 MiB work; invalid sizes and `full + prefixSize` fail validation |
| Early decision | A blocked prefix produces zero upstream headers/body; an unfinished prefix upload cannot dispatch upstream |
| Exact forwarding | N-1, N, N+1, and much larger bodies preserve length, digest, order, and single delivery, with N equal to the selected window |
| Boundary matching | Markers inside, ending at, crossing, and beyond N obey prefix scope; explicit full inspection sees eligible matches beyond N |
| Memory | WAF-only prefix storage stays in memory through 1 MiB even when complete-body policy has a smaller spill threshold |
| Protocols | H1/H2 downstream crossed with H1/H2 upstream, H1 chunked bodies, and local 100 Continue; verify and report the registered-buffer trailer limitation separately |
| Lifecycle | Short-body EOF, cancellation, early upstream responses, retry refusal, and retained-source cleanup |
| Composition | Existing full-cache reuse, frozen body representation, incompatible later consumers, and independent physical caps |
| Reload | Changing `prefixSize` and switching prefix/full/detection-only affect new requests atomically; invalid generations are not published |

## Delivery status

Local implementation exists in Edgion and the sibling Pingora branch
`feat/waf-request-body-prefix`. Local checks and independent reviews are recorded
in the tracking task. The Gateway wire/hot-reload matrix has not run against the
shared test topology. Publishing the fork through the supported source policy
and updating the final dependency revision also remain pending. This spec does
not claim those acceptance gates have passed.

## References

- [Body handling and project rules](../../AGENTS.md)
- [Gateway plugin execution](../../skills/01-architecture/02-gateway/05-plugin-system.md)
- [Prefix admission review contract](../../skills/04-review/http-body/regression-guards/waf-prefix-before-dispatch.md)
- [WAF user configuration](../../docs/en/user-guide/http-route/filters/edgion-plugins/waf.md)
