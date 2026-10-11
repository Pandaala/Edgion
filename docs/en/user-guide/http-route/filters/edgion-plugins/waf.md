# Native WAF

Edgion provides a runtime-independent WAF engine implemented in Rust. It uses
bounded SecLang compilation and transaction execution and does not link to an
external WAF engine. Coraza, ModSecurity, and OWASP CRS are compatibility
references and test inputs only.

The WAF is one top-level `spec.waf` configuration. Rules live in typed
`WafRuleBundle` EdgionConfigData resources; the plugin references a typed `WafPolicy` EdgionConfigData that selects one
named profile of ordered bundle references. Do not copy the WAF into the four ordinary
plugin stage arrays.

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
    policyRef:
      name: application-waf-policy
    activeProfileRef:
      name: application-waf-selector
```

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: application-waf-policy
  namespace: default
spec:
  data:
    type: WafPolicy
    config:
      profiles:
        baseline:
          bundleRefs:
            - name: application-rules-v1
              expectedDigest: a30e47998cace9b73f037d44f3fc9f0a29614e1753e07e6ea9f5c4e4c4d6b22d
      defaultProfile: baseline
```

Reference this resource from an `HTTPRoute` with the usual Gateway API
`ExtensionRef` filter.

## Configuration

| Field | Required | Default | Meaning |
|-------|----------|---------|---------|
| `policyRef.name` | yes | — | Fixed reference to a `WafPolicy` EdgionConfigData |
| `policyRef.namespace` | no | EdgionPlugins namespace | Policy namespace; cross-namespace use requires visibility or ReferenceGrant |
| `activeProfile` | no | `WafPolicy.defaultProfile` | Fixed profile name for this plugin; mutually exclusive with `activeProfileRef` |
| `WafPolicy.profiles` | yes | — | One to 16 named profiles; each contains one to 16 ordered `bundleRefs`, with at least one required reference |
| `WafPolicy.defaultProfile` | yes | — | Profile used without a usable Selector choice |
| `activeProfileRef` | no | — | Per-plugin reference to a typed `Selector` EdgionConfigData for hot switching among configured profiles; mutually exclusive with `activeProfile` |
| `WafPolicy.profiles.<name>.bundleRefs[].name` | with each reference | — | Target WafRuleBundle EdgionConfigData name |
| `WafPolicy.profiles.<name>.bundleRefs[].namespace` | no | WafPolicy namespace | Target namespace; cross-namespace use follows `visibility` and `ReferenceGrant` |
| `WafPolicy.profiles.<name>.bundleRefs[].expectedDigest` | no | — | Optional lowercase SHA-256 content pin; when omitted, valid bundle updates take effect automatically |
| `WafPolicy.profiles.<name>.bundleRefs[].optional` | no | `false` | Skip a missing or invalid independent addition and report `PartiallyInvalid`; never mark the base bundle optional |
| `mode` | no | `on` | `on` enforces disruptive actions; `detectionOnly` records matches without blocking |
| `priority` | no | `0` | Tie breaker within one attachment scope from `0` through `65535`; a larger value wins |
| `requestBody.inspection` | no | `full` | Inspect a bounded prefix before backend dispatch, or use `full` for complete-body inspection |
| `requestBody.prefixSize` | no | `64KiB` | Prefix window from 1 byte through `1MiB`, using size strings such as `16KiB`, `100k`, or `1m`; only valid with `prefix` |
| `requestBody.maxInspectSize` | no | Gateway physical cap | Positive source-byte window; full mode only |
| `requestBody.onLimit` | no | `continue` | Stop remaining WAF phases on inspection/storage policy cutoff, or `reject` with 413; full mode only |
| `requestBody.json` | no | engine defaults | Complete JSON checker limit overrides; only valid with `full` |

For a disruptive `deny` or `block` action, `status` must be a final HTTP status
from `200` through `599`. Informational responses, including `101 Switching
Protocols`, cannot be used as local WAF replies. The same range applies to
phase default deny statuses; an out-of-range value rejects the ruleset during
compilation.

Rule source is never emitted through WAF errors, plugin logs, response bodies,
metrics, or Rust `Debug`. Gateway admin configuration output replaces it with
`[redacted]`; controller-to-gateway configuration sync still carries the real
source.

## Static rule selection

Use YAML conditions in each profile's `ruleSelection` to select rules from trusted
configuration. Controller validates the policy and Gateway compiles it again when
publishing a generation. Requests use the compiled result. This example excludes
only rules matching the tag, phase and ID range while retaining `always-required`:

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: application-waf-policy
  namespace: default
spec:
  data:
    type: WafPolicy
    config:
      defaultProfile: application
      profiles:
        application:
          bundleRefs:
            - name: application-rules-v1
          ruleSelection:
            exclude:
              allOf:
                - tag: application-specific
                - phase: 2
                - ruleIdRange: {start: 100000, end: 100999}
                - not: {tag: always-required}
```

Missing `include` means `true`; missing `exclude` means `false`. The result is
`include && !exclude`: exclusion wins and source-removed rules stay removed.
Omitting `ruleSelection` or using `{}` retains the original compilation path;
`null` is invalid.

A condition is a boolean or an object with exactly one key: `tag`, positive
`ruleId`, inclusive `ruleIdRange: {start, end}`, `phase: 1..5`, a nonempty
`allOf` / `anyOf` list, or one `not` condition. Tags match exact case-sensitive
literals, without wildcards, regex, macros or whitespace normalization.
Selection applies to an entire chain. IDs and phases come from the root; tags
come from validated members. Structural markers are retained. The current strict
source language forbids authored tags on chain children.

Both conditions share limits of 256 nodes, 64 metadata conditions and depth 16.
Tags are limited to 256 UTF-8 bytes. Compact JSON is limited to 16 KiB per profile
and 64 KiB per policy. Unknown keys, wrong types, empty groups and duplicate keys
visible to direct decoding are rejected. Resource ingestion normalizes through a
JSON object first, which can collapse raw duplicate keys; do not rely on them.

Exclusion cannot hide invalid rules. The original rules must pass syntax, semantic,
regex, phrase-asset and request-body policy validation. Unmatched tags and valid
empty selections may publish; Accepted messages contain fixed numeric summaries.
They do not independently trigger `PartiallyInvalid`. Updating only selection
conditions hot-reloads the rules; in-flight requests retain their previous generation.

Choose language profiles from a trusted inventory of the entire serving stack.
Do not simply exclude every `language-php` or `language-java` rule: some also
protect generic paths, SSRF or attacks crossing language boundaries. The repository's
`config/waf/crs-4.25.1-supported-v1/language-selection-examples/` contains audited,
version-specific Java/PHP examples and an explicit omission ledger. Confirm their
backend assumptions before use and repeat the audit after ruleset updates.

## Inspect local tags

`waf tags` accepts one to 16 local WafRuleBundle EdgionConfigData YAML files.
The `--selection` file contains only a standalone include/exclude object:

```bash
edgion-cli waf tags --bundle bundle.yaml --format json
edgion-cli waf tags --bundle setup.yaml --bundle core.yaml \
  --selection selection.yaml --selected --offset 0 --format json
edgion-cli waf tags --bundle core.yaml --tag language-php --format json
```

```yaml
include: true
exclude:
  allOf:
    - tag: application-specific
    - not: {ruleId: 100001}
```

Reports distinguish semantic inventory, planned selection and successfully compiled
membership. Invalid rules may still have complete tag inventory, while
`baselineCompilationPassed` / `selectedCompilationPassed` report failure or unknown
accurately. Local inspection does not resolve cluster references or validate a
plugin's body mode; `bodyAdmissionChecked` is always `false`. Tag queries use
`semanticInventory`; successful selection queries use `compiledSelection`.

All input files together are limited to 64 MiB and the selection file to 64 KiB.
Reports contain at most 128 tag rows and 128 root ID references in 64 KiB of output;
omission counts do not change actual selection. Use `--offset` on a single tag,
`--selected` or `--excluded` query to page roots in ascending ID order. Rule source,
regex operands, phrase contents and internal Include paths stay out of the report.

## Shared rule bundles

Use a typed `EdgionConfigData` when a ruleset needs `Include` or `@pmFromFile`,
or when several WAF configurations share the same rules. A bundle carries
ordered root files, named rule files, and named phrase assets as one versioned
resource. This example uses a root file that includes a second file; the second
file reads a phrase asset from the same bundle:

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: application-rules-v1
  namespace: default
spec:
  enable: true
  data:
    type: WafRuleBundle
    config:
      version: "2026.1"
      profile: baseline
      provenance: operations/revision-1
      digest: a30e47998cace9b73f037d44f3fc9f0a29614e1753e07e6ea9f5c4e4c4d6b22d
      roots: [main.conf]
      rules:
        - name: main.conf
          content: |
            Include child.conf
        - name: child.conf
          content: |
            SecRule REQUEST_URI "@pmFromFile phrases.data" "id:2001,phase:1,deny,status:403"
      phraseAssets:
        - name: phrases.data
          content: |
            /blocked
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: application-waf
  namespace: default
spec:
  waf:
    policyRef:
      name: application-waf-policy
```

| Bundle field | Required | Meaning |
|--------------|----------|---------|
| `version`, `profile`, `provenance` | yes | Non-empty version, ruleset profile, and source description included in the digest |
| `digest` | no | Optional lowercase SHA-256 self-check of the canonical bundle; omit it for directly managed rules |
| `roots` | yes | Ordered rule member names to compile; at least one root is required |
| `rules[].name`, `rules[].content` | yes | Relative member path and SecLang source; at least one rule member is required |
| `phraseAssets[].name`, `phraseAssets[].content` | no | Relative member path and text for `@pmFromFile` |

The digest covers `version`, `profile`, `provenance`, ordered `roots`, and
rule and asset members sorted by name within each group. Each string is framed
by an 8-byte big-endian length; each list starts with an 8-byte big-endian item
count. Hash the bytes after the `edgion-waf-rule-bundle-v1` prefix and a zero
byte. Member names must be safe relative paths: no empty, `.` or `..` segment,
backslash, colon, or control character. Names are unique across rule and asset
members; every root names a rule member. The bundle has fixed member and byte
limits, and the complete selected generation must pass strict compilation.

The selected profile compiles bundles in `bundleRefs` order and each bundle's
`roots` order. Includes and phrase assets resolve only within the bundle that
contains the requesting rule. Duplicate rule IDs across all selected bundles
reject the entire generation.
The Controller checks each reference and any configured content pin, and the Gateway checks the
transported contents again before publishing the generation. An invalid,
missing, disabled, wrong-type, or unauthorized reference cannot publish a
partial base ruleset. An optional addition can be skipped with
`PartiallyInvalid=True` when absent or invalid; the combined generation still
must compile strictly. A broken required bundle retains the last valid
generation. `WafRuleBundle` does not use the fail-to-base overlay policy.

To keep several versions or sensitivity levels available, give each combination
a named profile. For example, `low`, `mid`, and `high` can reference
different versioned bundles and scoring parameters. These names are operator
choices, not built-in CRS levels; review the exact rule IDs and thresholds in
each bundle. Multiple bundle references in one profile aggregate in declaration
order. Only the selected profile runs for any request.

For dynamic selection, set `activeProfileRef: {name: application-waf-selector}`
on `EdgionPlugins.spec.waf` and create an EdgionConfigData with `data.type: Selector` and
`data.config.active: high`. Changing `active` switches to another configured
profile after Controller validation. A missing, disabled, wrong-type, or unknown
selection falls back to `defaultProfile`; it cannot disable WAF.
For a fixed choice, set `activeProfile: high` on the plugin instead. The name
must exist in the policy. Omit both selection fields to use `defaultProfile`.

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: application-waf-selector
  namespace: default
spec:
  data:
    type: Selector
    config:
      active: high
```

For pinned references, bundle content changes do not activate automatically:
create a new versioned bundle resource, calculate its digest, then update each
WafPolicy reference's `name` and `expectedDigest` to opt in. Keep the prior
versioned resource while any consumer still pins it. Without `expectedDigest`,
a valid bundle update activates after Controller and Gateway validation.
When cross-namespace reference validation is enabled, a target with
`visibility: Namespace` needs a matching `ReferenceGrant` in the target
namespace; `visibility: Cluster` allows references from all namespaces.
Writers of a referenced bundle can change executable WAF policy, so restrict
write access accordingly. Admin output and Rust `Debug` redact rule and asset
bodies, while trusted configuration sync carries them to the Gateway.

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
For profile `bundleRefs`, the local command has no Controller-resolved bundle contents:
it reports an unresolved-reference failure rather than claiming publication
readiness. Apply the resources and inspect their status for the full result.

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

Put these directives in a referenced WafRuleBundle member and set
`requestBody.inspection: full` on the WAF plugin:

```text
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

Put these directives in a referenced WafRuleBundle member and set
`requestBody.inspection: full` on the WAF plugin:

```text
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
  whole-body-dependent rules require `inspection: full` (the default). Validation
  checks the entire generation, including later-phase body dependencies.
- In explicit prefix mode, WAF alone captures only its inspection window in the shared request-body
  storage, entirely in memory, including a 1 MiB window. This bounded prefix
  uses its window as both memory and total capacity, independently of the
  complete-body spill threshold.
  After admission the original prefix is sent once,
  followed by the unread remainder. A pre-existing complete body cache is reused.
  Body mutation after prefix inspection and incompatible streaming processors
  fail closed. Native prefix captures disable upstream retries, including short
  bodies; complete shared captures retain their existing retry policy.
- Full-mode rules run only after a complete document has been inspected. An
  optional `maxInspectSize` sets a smaller inspection window. With no other
  complete-body consumer, the new storage capacity can also bound this window.
  One additional byte proves truncation; exactly the window size requires real
  EOF to count as complete. Incomplete JSON/XML results are discarded, and all
  remaining WAF phases stop. `onLimit: continue` forwards the original body;
  `onLimit: reject` returns 413 even in `detectionOnly`. Phase 1 findings remain.
  The access log records `inspection_stop` with the admitted byte count and
  `inspection` or `storage` reason. A cutoff is not a complete clean verdict.
  Complete-body co-consumers still require complete shared capture; their storage
  failures cannot be softened by this WAF policy. Existing complete caches retain
  their original capacity. Real parser, transport and storage failures remain
  failures. An explicit window above the pinned Gateway physical cap is rejected
  at runtime. Omit `prefixSize` in full mode.
- The gateway physical body cap remains independent of the inspection window.
  It also bounds ordinary HTTP without WAF or body capture, even when capture
  is disabled. Canonical gRPC, CONNECT and successfully upgraded tunnel traffic
  are excluded; an Upgrade request header alone is not an exemption.
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

Optional `requestBody.maxJsonBodySize` and `requestBody.maxXmlBodySize` impose
hard request-size ceilings for the effective processor selected after Phase 1.
Both accept positive byte-size strings, for example `2MiB`. Omission adds no
format-specific restriction; the Gateway body ceiling remains independent.
These limits apply in both `prefix` and `full` mode, in `detectionOnly`, and even
when no rule reads the body. They do not change the inspection window or enable
content sniffing/decompression. Only the selected WAF configuration applies.

A format hard limit checks native received bytes and any current logical cached
body, prevents later body mutation, and is incompatible with mutating streamed
body handlers in either installation order. It continues counting native input
after prefix admission without counting replay twice. Overflow returns 413 when
no final response has been selected; otherwise the existing response pipeline
terminates the remaining upload without replacing an already-selected response.
Earlier backend exposure cannot be undone. Complete prebuffering is needed for
zero upstream exposure. General inspection-window continuation is a separate
policy; format hard limits always reject excess.

The engine has fixed limits for source size, rule counts, regex automata,
matcher scratch, transaction collections, transformations, body decoding,
findings, and retained bytes. Limit failures are closed failures; they never
fall back to an unbounded path.

## Bounded JSON inspection

Full request-body inspection always uses the native JSON checker for effective
JSON processors. Omitting `requestBody.json` selects deterministic defaults;
this field supplies a complete override profile. Zero is restrictive, and every
field in an explicit profile is required. Legacy `document`, `plan` and `facts`
fields are rejected.

The Gateway feeds actual capture chunks to the checker and proves bounded
completion before Phase 2. Cached bodies use bounded immutable snapshot chunks;
JSON-only spilled bodies need no complete materialization. The checker
uses one grammar pass, matches precompiled rules as values complete,
and retains only required names, matching results and explicitly named deferred
values. It does not build a complete JSON value tree. Raw REQUEST_BODY visibility
remains controlled by the existing processor and ctl rules.

Lexical and structural retained-byte limits count dynamic heap capacity, excluding
fixed checker storage. Keys and paths each have 256 bytes of inline scratch, and
token decoding has 1024 bytes; these are shared across nesting levels. Spill
allocations are fallible and fully charged. A zero heap budget can still admit
inline data, while source, decoded-byte, key, path, depth and work limits remain
independently enforced. Rule selection scratch is reused and counts toward the
runtime aggregate retained-byte limit, including while evaluating a chain branch.

| Domain | Default limits |
|---|---|
| lexical | source: effective inspection ceiling; raw token 16 MiB; decoded/retained 32 MiB; events 131072; depth 64; work 128 MiB |
| structure | retained 8 MiB; keys 4 MiB; members 65536; path 16 KiB; depth 64; work 128 MiB |
| results | candidates 65536; facts 1000000; retained 32 MiB; work 128 MiB; deferred entries 4096; deferred bytes 4 MiB |

```yaml
requestBody:
  inspection: full
  json:
    lexical:
      maxRawTokenBytes: 16777216
      maxDecodedBytes: 33554432
      maxEvents: 131072
      maxDepth: 64
      maxRetainedBytes: 33554432
      maxWork: 134217728
    structure:
      maxRetainedBytes: 8388608
      maxKeyBytes: 4194304
      maxMembers: 65536
      maxPathBytes: 16384
      maxDepth: 64
      maxWork: 134217728
    results:
      maxCandidates: 65536
      maxFacts: 1000000
      maxRetainedBytes: 33554432
      maxWork: 134217728
      maxDeferredEntries: 4096
      maxDeferredBytes: 4194304
```

`results.maxFacts` bounds allocated candidate/profile/change/outcome/capture
record slots, including reusable free slots and spare vector capacity. It does
not count only matching rules or live matches. Name/value storage, state bitmaps
and the named-demand index instead consume the retained-byte/work budgets.

Physical inspection limits are independent of logical WAF and Gateway body
limits. A physical failure rejects in DetectionOnly too and cannot be retried
through another parser. Malformed JSON retains the existing DetectionOnly
metadata behavior. Duplicate keys replace earlier subtrees; malformed syntax and
physical work already consumed cannot be erased by later duplicate replacement.
Full configurations with unsupported broad dynamic JSON value dependencies are
rejected during validation, including configurations using default JSON limits.
Use finite named dependencies where later transaction values are required.

Logical publication checks use fixed priority: source/prelude, name length,
depth, entries, decoded bytes, traversal work, then combined totals. Each limit
remains independent; competing failures may differ from the former DOM parser.
Depth is independently configurable; there is no historical 127-container guard.
The WAF runtime-byte gauge combines logical transaction accounting with known
provisional parser/result heap capacities during inspection. It excludes source
storage and allocator overhead and does not measure RSS.

## Bounded XML inspection

Effective XML processors also use native inspection with `requestBody.inspection:
full`. Automatic selection recognizes `application/xml`, `text/xml` and media
types ending in `+xml`, ignoring case and parameters. A phase-1
`ctl:requestBodyProcessor=XML` can override selection. HTTP Content-Type parameters,
including `charset`, do not drive transcoding or XML encoding validation. Input
bytes must be UTF-8; an XML declaration's encoding must be UTF-8 when present.
Use your existing policy resource with complete inspection:

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
      inspection: full
    policyRef:
      name: application-waf-policy
```

In the referenced rule bundle, `XML` selects emitted text and attribute values,
`XML:/*` selects text/CDATA, and `XML://@*` selects attributes. For example:

```apache
SecRule XML:/*|XML://@* "@contains forbidden" "id:100001,phase:2,deny,status:403"
```

These selectors use the engine's value projection, not arbitrary XPath. Normal
text is emitted separately between markup and trims outer literal XML whitespace;
CDATA is separate and untrimmed. Attributes use their lexical qualified names,
including namespace declarations. Matched names are `XML` or `XML:<attribute>`.
Comments, processing instructions and element names are not values. XML does not
add ARGS entries; combined-size metadata includes query arguments only.

The supported profile is UTF-8/XML 1.0 with optional BOM/declaration. It validates
structure, characters, namespace bindings and duplicate attributes, normalizes
line endings and attribute literal whitespace, and accepts the five predefined
entities plus valid numeric references. DTD, external/general entities, non-UTF-8
bytes/declarations and unsupported XML paths are rejected. This is not schema validation.
Dynamic XML value operands fail Full validation; dynamic count operands remain
supported. Existing CRS attribute exclusions still apply in rule order.

| Domain | Fixed default limits |
|---|---|
| parser | source: effective inspection ceiling; raw token 16 MiB; name 16 KiB; decoded bytes 32 MiB; events 262144; depth 64 |
| structure | attributes per tag 65536; active namespace bindings 65536; active names/namespaces 8 MiB |
| storage/work | parser heap 32 MiB and work 128 MiB; selected-result limits match the JSON result defaults |

JSON and XML source-byte ceilings come from the effective inspection window,
which inherits the Gateway physical cap (32 MiB by default). There is no
`json.lexical.maxSourceBytes` operator field. Independent token, heap and work
budgets still apply. There is no `requestBody.xml` configuration field. Parser safeguards remain
independent of logical WAF and Gateway body limits. Contiguous ordinary values
are borrowed; cross-chunk or normalized values use reusable inline scratch with
bounded growth. The engine keeps no complete XML tree or extracted value map.
Required matching/count metadata and captures can still consume bounded storage.

Full XML uses the same canonical BodyStorage capture and replay path as JSON;
XML-only spill needs no complete materialization. Raw visibility and malformed
DetectionOnly fallback retain existing policy; malformed input discards provisional
XML matches. Physical failures reject even in DetectionOnly. Empty transport
bodies are accepted, while nonempty whitespace/comment-only documents need a root.
Phase-2 capture, chain, TX and control effects derived from XML values run after
complete admission in rule order. Phase-1 effects execute before body inspection.

## Reload behavior

Each update compiles a complete immutable generation before publication. A
valid update is atomically visible to new requests. In-flight requests retain
their original generation. An invalid update reports `Accepted=False` and
keeps the last activatable generation; deleting the EdgionPlugins resource
removes it for new resolution without invalidating already-held runtime arcs.

Only one WAF generation owns a request. Edgion selects it before phase 1 from
Route, Gateway, then global candidates. Within the selected scope, larger
`priority` wins; equal priorities use declaration order.
Backend-scoped WAF configuration is rejected because backend selection happens
after the request WAF phase. The Controller reports the owning HTTPRoute or
GRPCRoute as `Accepted=False` with reason `UnsupportedValue`; changing the
referenced EdgionPlugins generation automatically revalidates the Route.

## Current compatibility boundary

The production compiler accepts only behavior implemented by Edgion's bounded
runtime profile. It rejects unsupported directives, operators, actions,
unresolved external includes/assets, and full-response-body requirements instead of
silently ignoring them. Compatibility is therefore an explicit per-ruleset
compile result, not a claim that every historical ModSecurity configuration is
accepted.

## Repository-supported CRS 4.25.1

You can import the complete YAML resources in
`config/waf/crs-4.25.1-supported-v1/generated/` into `edgion-default`, then
attach `EdgionPlugins/crs-supported` with a route ExtensionRef. The example
selects `standard` (PL1); `strict` selects PL4. Both explicitly inspect XML
attributes and validate UTF-8. Start with `mode: detectionOnly` to assess valid
application traffic before enabling `on`. The policy pins required tuning and
core bundles; regenerate and deliberately update pins when changing content.

The package keeps 565 of 627 original CRS core units, original setup 900990
and one separate Edgion tuning unit (567 total). It excludes 59 response-body
units and original 901320/905100/905110: persistent collection initialization
and Apache probe audit/bypass controls remain unsupported. Loopback probe
User-Agents receive ordinary inspection. Retained original rule bytes and all
21 phrase assets are pinned with upstream attribution. This is an explicit
supported source set, not full CRS compatibility.

Original initialization provides thresholds 5/4 and early_blocking=0; no
implicit argument-length cap is added. Strict PL4 can block a backend 500
through response-status rules before response headers are committed; standard
PL1 does not enable that PL2 predicate. No upstream response body is inspected.
Runtime budgets remain unchanged, so large bodies may still fail a work or
memory limit. DetectionOnly does not suppress fatal runtime failures.

Reproduce the package from repository sources with
`cargo run -p edgion-resources --example generate_supported_crs -- --check`.
The package README and `generated/acceptance.json` describe exact exclusions,
source pins, profile settings and output digests.
