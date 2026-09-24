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

For a disruptive `deny` or `block` action, `status` must be a final HTTP status
from `200` through `599`. Informational responses, including `101 Switching
Protocols`, cannot be used as local WAF replies. The same range applies to
phase default deny statuses; an out-of-range value rejects the ruleset during
compilation.

Rule source is never emitted through WAF errors, plugin logs, response bodies,
metrics, or Rust `Debug`. Gateway admin configuration output replaces it with
`[redacted]`; controller-to-gateway configuration sync still carries the real
source.

## Inspect rule support before applying

Run the local CLI on one `EdgionPlugins` YAML document:

```sh
edgion-cli waf inspect --file application-waf.yaml --format json
```

The command needs no Controller connection or token. It reads at most 16 MiB and
returns a versioned report. `analysisComplete` means the SecLang source reached
the compatibility analysis stage; it does **not** mean the rules can be
published. Only `strictValidationPassed: true` means the exact WAF configuration
passed the same strict compiler and request-body inspection checks used by the
Controller and Gateway. The command exits nonzero otherwise.

For example, a rule using complete upstream response-body matching reports a
`full_response_body` finding with its rule ID and zero-based document and
directive indexes. `reason` tells the operator to remove that rule. A rule
without an ID stops analysis with a safe physical line and an instruction to
add an ID; no ID is invented. `unattributedFindings`
counts any findings that cannot be assigned to an ID, so `unavailableRuleCount`
does not claim to include them. Syntax, missing-asset,
unsupported-semantics, and resource-limit failures have a fixed `category` and
safe location when one is available. The report never contains source lines,
regexes, operands, or file identities. `omittedFindings` and
`omittedRuleSummaries` show whether the 128-entry presentation bounds hid any
entries. An incomplete analysis reports no compatibility counts; correct the
reported failure and inspect again.

The public `WafConfig::inspect_support()` result in `edgion-resources` is the
same value-free report the CLI presents. Other tools, including EdgionCenter,
can call it without duplicating SecLang parsing; they must enforce their own
input-byte limit and access control before presenting it.

## SQL injection and XSS operators

`@detectSQLi` and `@detectXSS` are parameterless SecLang operators backed by
Edgion's native Rust implementation of the libinjection-go v0.3.2 behavior.
They inspect each selected value after its configured transformations. For
example:

```yaml
requestBody:
  inspection: full
rules: |
  SecRule ARGS_GET "@detectSQLi" "id:1101,phase:1,deny,status:403,t:urlDecodeUni,capture"
  SecRule ARGS_GET "@detectXSS" "id:1102,phase:1,deny,status:403,t:urlDecodeUni"
```

When `capture` is set on a positive SQL injection match, the detected
fingerprint is available as `TX.0` for the rule's actions and later rules.
XSS detection does not create a capture or replace an existing `TX.0` value.
Negated matches do not capture. A failed chain publishes neither its finding
nor its capture. Rule targets, transformations, and chain behavior follow the
same transaction semantics as other operators.

These detectors scan complete selected values, so a rule consuming
`REQUEST_BODY` or body-derived arguments needs `requestBody.inspection: full`.
The prefix profile also requires `full` when a rule uses `capture`, including
header-only rules.
The detector input limit is 4 MiB per value. Operator work is charged to the
request's bounded runtime budget, including all SQL dialect/quote or HTML
contexts. Reaching a limit closes the WAF request according to its normal
runtime error policy. The implementation does not use Hyperscan.

## Execution and limits

### Request-scoped control actions

With `requestBody.inspection: full`, the strict compiler accepts four `ctl`
actions: `forceRequestBodyVariable=On|Off`,
`requestBodyProcessor=URLENCODED|MULTIPART|JSON|XML`,
`ruleRemoveByTag=<tag>`, and
`ruleRemoveTargetByTag=<tag>;<target>`. They change only the current request's
transaction. Other `ctl` actions, including audit and engine controls, remain
unsupported and reject the complete ruleset. Prefix inspection rejects all
`ctl` actions.

Body controls must run in phase 1, before request-body publication. The
processor action overrides the request's body parser; the force action controls
whether raw `REQUEST_BODY` is exposed alongside parsed values. `On` exposes
raw bytes even for structured processors; `Off` hides raw bytes. Neither action
changes the configured inspection mode or the physical body limit. Tag and
target exclusions take effect after the controlling rule matches, so earlier
rules remain evaluated. Target exclusions select only the named collection and
target of later tagged rules, including chain members. Failed chains commit no
controls. Each transaction has a bounded control-entry count and work budget;
exhaustion follows the normal WAF runtime failure policy.

```yaml
requestBody:
  inspection: full
rules: |
  SecRule REQUEST_HEADERS:X-Waf-Exempt "@streq yes" "id:1201,phase:1,pass,ctl:ruleRemoveByTag=managed-check"
  SecRule ARGS "@contains blocked" "id:1202,phase:2,deny,status:403,tag:managed-check"
```

- Phase 1 inspects the entry request URI, method/protocol, client address, and
  ordered request headers. A phase-1 denial occurs before request-body capture.
- With `requestBody.inspection: prefix`, phase 2 inspects at most the first
  `prefixSize` payload bytes (64 KiB by default, at most 1 MiB) before backend
  selection or connection. Transfer framing
  is removed; `Content-Encoding` is not decompressed. A matching blocking rule
  sends no request headers or body to the backend. Admission only means no rule
  blocked this prefix; bytes after the window are not inspected by WAF.
- Prefix mode accepts a conservative subset of positive raw `REQUEST_BODY`
  `@contains` / `@pm` rules. Phase-2 rules using only headers, URI, or query-only
  collections (`ARGS_GET` / `ARGS_GET_NAMES`) use normal rule evaluation without
  reading the body. If no rule consumes body values, neither inspection mode
  captures it. Mixed header/body rules retain their configured order.
  Negated body matches, structured body targets, chains, control flow, and
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
- When rules consume body values, `inspection: full` waits for complete bounded capture and uses the existing
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
