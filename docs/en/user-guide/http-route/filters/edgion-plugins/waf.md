# Native WAF

Edgion provides a runtime-independent WAF engine implemented in Rust. It uses
bounded SecLang compilation and transaction execution and does not link to an
external WAF engine. Coraza, ModSecurity, and OWASP CRS are compatibility
references and test inputs only.

The WAF is one top-level `spec.waf` configuration. Do not copy it into the four
ordinary plugin stage arrays.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: application-waf
  namespace: default
spec:
  waf:
    mode: on
    priority: 100
    requestBody:
      inspection: prefix
      prefixSize: 64KiB
    rules: |
      SecRule REQUEST_URI "@contains /admin" "id:1001,phase:1,deny,status:403"
      SecRule REQUEST_BODY "@contains forbidden" "id:1002,phase:2,deny,status:406"
```

Reference this resource from an `HTTPRoute` with the usual Gateway API
`ExtensionRef` filter.

## Configuration

| Field | Required | Default | Meaning |
|-------|----------|---------|---------|
| `rules` | yes | — | Inline SecLang source compiled as one immutable generation |
| `mode` | no | `on` | `on` enforces disruptive actions; `detectionOnly` records matches without blocking |
| `priority` | no | `0` | Owner-selection priority from `0` through `65535`; a larger value wins |
| `requestBody.inspection` | no | `prefix` | Inspect a bounded prefix before backend dispatch, or use `full` for complete-body inspection |
| `requestBody.prefixSize` | no | `64KiB` | Prefix window from 1 byte through `1MiB`, using size strings such as `16KiB`, `100k`, or `1m`; only valid with `prefix` |

Rule source is never emitted through WAF errors, plugin logs, response bodies,
metrics, or Rust `Debug`. Gateway admin configuration output replaces it with
`[redacted]`; controller-to-gateway configuration sync still carries the real
source.

## Execution and limits

- Phase 1 inspects the entry request URI, method/protocol, client address, and
  ordered request headers. A phase-1 denial occurs before request-body capture.
- With `requestBody.inspection: prefix`, phase 2 inspects at most the first
  `prefixSize` payload bytes (64 KiB by default, at most 1 MiB) before backend
  selection or connection. Transfer framing
  is removed; `Content-Encoding` is not decompressed. A matching blocking rule
  sends no request headers or body to the backend. Admission only means no rule
  blocked this prefix; bytes after the window are not inspected by WAF.
- Prefix mode accepts a conservative subset of positive raw `REQUEST_BODY`
  `@contains` / `@pm` rules. Negated, structured, chained, control-flow, and
  whole-body-dependent rules require explicit `inspection: full`. Validation
  checks the entire generation, including later-phase body dependencies.
- WAF alone captures only its inspection window in the shared request-body
  storage, entirely in memory, including a 1 MiB window. This bounded prefix
  uses its window as both memory and total capacity, independently of the
  complete-body spill threshold.
  After admission the original prefix is sent once,
  followed by the unread remainder. A pre-existing complete body cache is reused.
  Body mutation after prefix inspection and incompatible streaming processors
  fail closed. Native prefix captures disable upstream retries, including short
  bodies; complete shared captures retain their existing retry policy.
- `inspection: full` waits for complete bounded capture and uses the existing
  shared memory/disk storage before phase 2. Choose it when rules need complete
  body semantics; omit `prefixSize` in this mode. Physical body and execution
  limits still apply. It also applies to raw rules that previously ran while forwarding.
- The gateway physical body cap remains independent of the inspection window.
  Overflow detected after prefix admission can happen after backend exposure;
  it is not a pre-dispatch WAF rejection. `detectionOnly` suppresses rule-based
  blocking, but does not suppress transport, storage, or physical-limit failures.
- Phase 3 inspects upstream status and ordered response headers before they are
  sent downstream.
- The current production profile does not aggregate upstream responses and
  rejects rules requiring complete `RESPONSE_BODY` semantics at compile time.
  Response-safe phase-4 actions run with an explicit empty end-of-stream
  window. The main upstream response remains streaming.
- Phase 5 runs once for a normally completed response transaction. Findings
  expose bounded counts and rule IDs only; payloads and matched values are not
  logged.

The engine has fixed limits for source size, rule counts, regex automata,
matcher scratch, transaction collections, transformations, body decoding,
findings, and retained bytes. Limit failures are closed failures; they never
fall back to an unbounded path.

## Reload behavior

Each update compiles a complete immutable generation before publication. A
valid update is atomically visible to new requests. In-flight requests retain
their original generation. An invalid update reports `Accepted=False` and
keeps the last activatable generation; deleting the EdgionPlugins resource
removes it for new resolution without invalidating already-held runtime arcs.

Only one WAF generation owns a request. Edgion selects it before phase 1 from
the global, Gateway, and Route candidates. Larger `priority` wins; equal
priorities use Route over Gateway over global, and then declaration order.
Backend-scoped WAF configuration is rejected because backend selection happens
after the request WAF phase. The Controller reports the owning HTTPRoute or
GRPCRoute as `Accepted=False` with reason `UnsupportedValue`; changing the
referenced EdgionPlugins generation automatically revalidates the Route.

## Current compatibility boundary

The production compiler accepts only behavior implemented by Edgion's bounded
runtime profile. It rejects unsupported directives, operators, actions,
external includes/assets, and full-response-body requirements instead of
silently ignoring them. Compatibility is therefore an explicit per-ruleset
compile result, not a claim that every historical ModSecurity configuration is
accepted.
