# Plugin Conditions

> Shared `run` / `skip` predicates on `EdgionPlugins` entries (and on entry-level `dye` rules). This is not a standalone plugin.

Plugin conditions decide whether a plugin entry, a body gate, or a dye rule
runs for the current request. `skip` and `run` are action fields; each contains
an explicit `ConditionSet`. When both are present, `skip` is evaluated first.
A matching `skip` set skips the action, while a non-matching `run` set also
skips it.

```yaml
conditions:
  skip:
    anyOf:
      - type: keyExist
        key: { type: header, name: X-Internal-Request }
  run:
    allOf:
      - type: keyMatch
        key: { type: header, name: X-Env }
        values: ["production"]
```

## ConditionSet grammar

A set has exactly one top-level operator:

- `allOf` contains only direct Conditions and matches when every item matches.
- `anyOf` contains direct Conditions or one-level `{ name?, allOf: [...] }`
  groups and matches when any alternative matches.

This bounded DNF grammar supports flat AND, flat OR, and `(A AND B) OR C`:

```yaml
conditions:
  run:
    anyOf:
      - type: directPeerIpMatch
        ips: [10.0.0.0/8]
      - name: authenticated-production
        allOf:
          - type: keyMatch
            key: { type: header, name: X-Env }
            values: [production]
          - type: keyExist
            key: { type: header, name: X-Authenticated-User }
```

The grammar is intentionally non-recursive. Arrays must be non-empty. A set
cannot contain both operators, and the following shapes are rejected:
`allOf -> anyOf`, `anyOf -> anyOf`, nested `allOf -> allOf`, deeper nesting,
unknown container fields, and a clause containing both `type` and `allOf`.
An optional group `name` must be nonblank and affects attribution only.

### Body gates and dye rules

Entry-level `conditions`, supported `body.conditions`, and dye-rule
`conditions` all use the same `PluginConditions` action container. For
example, this body gate applies only to requests with a body-producing method:

```yaml
body:
  maxBodySize: 1MiB
  conditions:
    run:
      anyOf:
        - type: keyMatch
          key: { type: method }
          values: [POST, PUT, PATCH]
```

Body conditions decide whether the generic body gate applies; they do not
skip the inner plugin. Some intrinsic fail-close body plugins deliberately
reject `body.conditions`; their plugin-specific page documents that boundary.
Dye conditions decide only whether an already-matched outcome renders its
header. WebhookKeyGet has ordinary entry-level conditions, but does not gain a
body gate: its outbound webhook body is unrelated to the proxied request body.

## Variants

| `type` | Meaning |
|--------|---------|
| `keyExist` | True when the key is present. |
| `keyMatch` | True when the key's value is in `values` **or** matches `regex`. `negate: true` is the exclude spelling. |
| `timeRange` | True when now is on or after `after` and strictly before `before` (RFC3339). Either bound may be omitted. |
| `probability` | True with probability `ratio` (0.0–1.0). Optional `key` makes the sample deterministic. |
| `directPeerIpMatch` | True when the immediate TCP peer is in `ips` / `ipRefs`. |
| `remoteIpMatch` | True when the RealIp-resolved client is in `ips` / `ipRefs`. |
| `secretMatch` | True when the request key equals the selected Secret data value. `secretKey` names that data entry. |
| `hmacAuth` | True when the request passes header-only HMAC verification. HMAC fields (`secretGroups`, `algorithms`, …) are siblings of `type`. |
| `jwtClaimMatch` | True when a stored JWT claim matches `values` **or** `regex`. `claim` is a dot-nested path. |
| `mtlsMatch` | True when an mTLS client-certificate field matches. `field` is `subject`, `cn`, or `san`. |

There is no separate `include` / `exclude` type. A former `include` is `keyMatch`; a former `exclude` is `keyMatch` with `negate: true`.

`hmacAuth.validateRequestBody` is rejected on entry-level `run` / `skip`, body
gates, and dye conditions. It is valid inside RequestAccessPolicy and
RequestRestriction ConditionSets. An ordinary request-value miss or failed
HMAC verification is base false, then `negate` applies. An unresolved
ConfigData or Secret reference is different: it never matches and `negate`
never inverts it.

## ConfigData and Secret references

`directPeerIpMatch` / `remoteIpMatch` may list `ipRefs` to an IpList `EdgionConfigData`. `secretMatch` uses `secretRef` plus required `secretKey`; it resolves exactly `Secret.data[secretKey]`, never every value in the Secret. `hmacAuth` resolves `secretGroups[].secretRefs`. The Controller walks every direct and grouped Condition in entry, body, dye, RequestRestriction, and RequestAccessPolicy sets, and writes `resolvedIps` / `resolvedValues` / `resolvedCredentials` **in place** on those Condition objects.

- A missing, disabled, wrong-type, or unpermitted **IpList** `ipRefs` target is reported as `ResolvedRefs=False`; the Controller does not compile an empty matcher from unresolved `ipRefs`. A missing **Secret** referenced by `secretMatch` or HMAC `secretRefs` is reported the same way. For `secretMatch`, a missing, non-UTF-8, or empty selected `secretKey` value is also unresolved. These errors are included in the `edgion_controller_resource_abnormal` metric.

```yaml
- type: secretMatch
  key: { type: header, name: X-Origin-Token }
  secretRef: { name: origin-credentials }
  secretKey: token
```
- An unresolved entry-level or body-gate condition seals the EdgionPlugins runtime fail-closed, so requests receive the existing generic 500 response. An unresolved dye condition simply does not match.
- In `RequestRestriction`, an unresolved condition makes `Deny` mode return the generic 500 response. In `Allow` mode, the incomplete group is unavailable and evaluation continues with the remaining groups; if none matches, the normal configured rejection applies.
- In `RequestAccessPolicy`, an incomplete allow candidate does not match. Evaluation continues with sibling rules and candidates, and the policy denies normally if no complete candidate allows the request.
- RequestAccessPolicy keeps a separate partial-good rule for ConfigData **content** (bad CIDRs keep valid siblings; missing IP/URL ConfigData refs become `PartiallyInvalid` warnings). That sidecar rule does not apply to entry-level or RequestRestriction `ipRefs`.

## `jwtClaimMatch` and `mtlsMatch`

These variants do not use `KeyGet`. They read a list of candidate strings and succeed when **any** candidate matches `values` or `regex`.

**`jwtClaimMatch`** looks up `claim` in ctx `jwt_claims` first, then `oidc_claims`. Prerequisite: `jwt_auth` or `openid_connect` ran earlier with `storeClaimsInCtx: true`. An unmet prerequisite is a missing value (base false, then `negate`).

`claim` is required and must not be empty or whitespace-only. The same
configuration-time check applies in every Condition consumer.

```yaml
- type: jwtClaimMatch
  claim: realm_access.roles
  values: ["admin"]
```

**`mtlsMatch`** reads the exposed client certificate. Prerequisite: mTLS with a client certificate available on the session. An unmet prerequisite is a missing value (base false, then `negate`).

- `field: subject` / `field: cn` — 0 or 1 value
- `field: san` — any SAN entry matching satisfies. Optional `sanType` (`DNS`, `URI`, `Email`) filters by kind and strips the stored prefix (`uri:` / `email:`). `sanType: DNS` is the unprefixed remainder and therefore excludes `IP:`, `uri:`, `email:`, and `otherName:` entries. Untyped matching uses the raw stored strings.

```yaml
- type: mtlsMatch
  field: san
  sanType: URI
  values: ["spiffe://edgion.io/ns/foo"]
```

## Missing values and `negate`

Every variant has `negate` (boolean, default `false`).

1. A missing key is **base false** (the match itself did not succeed).
2. Then `negate` inverts the result. **Missing + `negate: true` is true** — this is the exclude-preserving missing-key verdict.

`keyExist` treats absence as false, then applies `negate`. `timeRange` and `probability` invert their computed result.

## `values` (not `value`)

Use `values: ["…"]` for exact matches. The singular `value` field is gone.

Matching is **exact**. There is no wildcard syntax in `values` (`/api/*` is the literal string `/api/*`). Prefix (and other) glob shapes belong in `regex`, for example `/api/*` → `^/api/.*`.

`values` and `regex` are OR-ed: the condition is true if either side matches.

`keyMatch`, `jwtClaimMatch`, and `mtlsMatch` must configure at least one
non-empty `values` or `regex` list. This is validated uniformly wherever a
condition appears, including plugin `run`/`skip`, body and dye conditions,
RequestRestriction sets, and RequestAccessPolicy supplemental sets.
If `values` is present, it must not be an empty list even when a non-empty
`regex` is also present.

## `caseSensitive`

Boolean, default **true**. Applies to **`values` only**. Regex case-folding is written in the pattern (`(?i)`).

When `caseSensitive: false`, exact matching uses ASCII-only folding (`eq_ignore_ascii_case`). Lists longer than 16 values are compiled into a HashSet with the same ASCII folding.

## Path normalization

`keyMatch` with `key.type: path` normalizes **exact `values` at load time** so they inhabit the same space as the request path. This is new for conditions; `RequestRestriction` already did it for `Path` sources.

`key.type: requestPath` is the entry-time **raw** path and is **never** normalized. Use it to match the form the client sent (`/%61dmin` stays `/%61dmin`; `path` sees `/admin`).

Regex patterns are **never** normalized. Write regex against the path form you actually observe.

## Compiled descriptors (access log)

Each compiled condition carries a runtime-only descriptor with its stable
location in the set, condition type, and accessor when applicable. Locations
distinguish direct leaves from nested groups, for example
`run.allOf[1]:keyMatch:hdr:X-Env` or a compact wire equivalent. Descriptors
never contain match values, regexes, CIDRs, claims, or secrets.

Match-only atomic descriptors are emitted only when one logical decision scope
contains more than one atomic Condition. For plugin conditions, the count spans
both `skip` and `run`; RequestRestriction counts its complete set; each
RequestAccessPolicy candidate counts its supplemental set. A zero- or one-leaf
scope is already explained by the plugin or policy outcome, so repeating the
successful leaf would add noise. Composite scopes retain matched descriptors
in configuration evaluation order so the successful branch or prefix can be
reconstructed.

This threshold never suppresses `cond_skip`, final allow/deny/reject outcomes,
named-group attribution, unresolved/configuration errors, or body-read errors.
Body-phase and dye evaluation remain silent regardless of leaf count.

## Example

```yaml
conditions:
  run:
    allOf:
      - type: keyMatch
        key: { type: path }
        values: ["/health", "/ready"]
        caseSensitive: true
      - type: keyMatch
        key: { type: path }
        regex: ["^/api/.*"]
      - type: keyMatch
        key: { type: path }
        values: ["/health", "/ready"]
        negate: true   # exclude spelling
      - type: keyExist
        key: { type: header, name: X-Canary }
        negate: true   # run when the header is absent
```

## Related

- [Request Restriction](./request-restriction.md) — standalone allow/deny plugin using the same ConditionSet
- [Request Access Policy](./request-access-policy.md) — L7 policy with supplemental ConditionSets
- [Dye](./dye.md) — entry-level dye rules reuse this `conditions` block
