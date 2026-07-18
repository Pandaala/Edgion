# Plugin Policy (Allowlist / Denylist)

The Controller can restrict which plugin types the Gateway is allowed to
execute, via an optional allowlist or denylist configured in
`validation.plugin_policy`. A restricted plugin is never instantiated by
the Gateway; depending on the configured action, requests that depend on
it are either **blocked** with a configurable status code or **allowed
through with the plugin skipped** — in both cases observably, via access
log, Gateway metrics, a resource status condition, and the Controller
abnormal-resource metric.

This document is the operator guide: what the policy does, how to
configure it safely, and the warnings you must read before enabling it.
For the field-by-field config schema, see
[Controller Configuration — validation.plugin_policy](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/02-config/00-controller-config.md#validationplugin_policy).

## How it works

- **Controller (identify + report)**: validates the policy at startup
  (fail-fast on any error), stamps matching plugin entries with an
  internal marker during preparse, and reports a dedicated
  `PluginPolicyRestricted` status condition listing the hits. It does
  **not** flip the entry's `enable` field and does **not** stop the
  resource from being cached or synced to the Gateway.
- **Gateway (enforce + observe)**: a stamped entry is never instantiated
  (no Wasm VM init, no external connections, etc.). A `Block` entry
  rejects the request; a `Bypass` entry is skipped and the request
  proceeds without it.

Policy absent (the default) means allow all: every compiled plugin type
remains usable by every resource, nothing is stamped, no condition is
emitted, and the Gateway takes the existing zero-overhead code path. The
feature only activates when an operator explicitly adds the
`plugin_policy` section below.

## Configuration

```yaml
validation:
  plugin_policy:
    mode: denylist       # denylist | allowlist
    plugins:
      - name: http/Wasm  # <family>/<Type>; shorthand uses default_action
```

Config-file only (no CLI flag). **Policy changes require a Controller
restart** — there is no hot reload for this section.

Entries are named `<family>/<Type>` across three families (`http`,
`stream`, `tls-route`). `denylist` restricts only the listed plugins;
`allowlist` restricts every plugin not listed, **globally** across all
three families — there is no per-family carve-out. The Controller
validates the policy at startup and refuses to start on a malformed
entry (unknown family/type, duplicate, out-of-range `block_status`, or
an `action`/`block_status` set on an `allowlist` entry). For the full
field reference, the qualified-name table, and the complete
allowlist/denylist semantics, see
[Controller Configuration — validation.plugin_policy](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/02-config/00-controller-config.md#validationplugin_policy).

## What the operator sees

### Resource status

When at least one entry in a resource is stamped, the resource gets a
dedicated condition:

```yaml
status:
  conditions:
    - type: PluginPolicyRestricted
      status: "True"
      reason: RestrictedByPolicy
      message: "requestPlugins[2] http/Wasm -> block(503); plugins[0] stream/ConnectionRateLimit -> bypass"
```

`Accepted` is not touched by this — it keeps meaning "resource valid and
synced". When a policy is relaxed (or removed) and the Controller
restarts, the next preparse actively removes a stale
`PluginPolicyRestricted` condition instead of leaving it wedged.

### Controller metric

`edgion_controller_resource_abnormal` classifies `PluginPolicyRestricted:True`
as an **err**-level trigger — both `block` and `bypass` hits count, since
any use of a restricted plugin is outside expectations regardless of the
action taken. This surfacing is **Kubernetes-mode only**: it depends on
the resource's live status having round-tripped through the watch. In
FileSystem/Etcd mode the scanner reads the pre-processing object and
never sees status conditions (a pre-existing property of this metric, not
specific to plugin policy).

### Gateway behavior — HTTP

- **Block**: the request is rejected with the configured status code and
  a fixed generic body, `{"message":"plugin disabled by policy"}` — the
  plugin name is never sent to the client. The block gate sits at the
  request stage and short-circuits the whole chain regardless of which
  stage the restricted plugin was actually configured in: a route that
  depends on a blocked response-stage plugin is failed closed at ingress
  rather than after a wasted upstream round trip. When several entries in
  a chain are blocked, the first one in spec order (`requestPlugins`,
  `upstreamResponseFilterPlugins`, `upstreamResponseBodyFilterPlugins`,
  `upstreamResponsePlugins`, in list order within each stage) determines
  the status code returned.
- **Bypass**: the plugin is skipped; the chain continues normally.
- Access log: a blocked request carries a `PluginPolicyBlock` plugin log
  entry naming the qualified plugin; a bypassed request carries a
  `PluginPolicyBypass` entry listing the skipped plugin(s). No hits, no
  entries.
- Metrics: `edgion_gateway_plugin_policy_blocked_total` and
  `edgion_gateway_plugin_policy_bypassed_total`, both labeled by the
  qualified plugin name.

### Gateway behavior — Stream / TLS-route

- **Block**: the connection is closed. There is no way to send an HTTP
  status pre-TLS or mid-stream, so `block_status` is ignored for
  `stream/` and `tls-route/` entries.
- **Bypass**: the plugin is skipped; the stream log records the marker,
  and the resource's `PluginPolicyRestricted` condition gives
  resource-level visibility.

## Operator warnings — read before enabling

1. **Bypassing an authentication/authorization plugin makes the affected
   routes less restrictive**, not more. If a route's security posture
   depends on a plugin (e.g. `JwtAuth`, `BasicAuth`, `IpRestriction`,
   `KeyAuth`), restrict it with `action: block`, not `bypass` — bypassing
   an auth plugin lets requests that would have been rejected reach the
   backend unauthenticated.
2. **This is execution control only.** Restricting a plugin type does not
   remove its code or its dependencies from the Gateway binary — it only
   prevents that plugin instance from being instantiated and run. Do not
   treat `plugin_policy` as a substitute for a build without a given
   plugin compiled in.
3. **Upgrade order matters.** Deploy the new Gateway binary (the one that
   understands the internal policy marker) to **every** Gateway in the
   fleet *before* enabling `plugin_policy` on the Controller. An older
   Gateway binary does not know about the marker field — serde silently
   skips the unknown key — and runs the plugin normally. That is
   **fail-open**: a plugin you believe is blocked on every Gateway may
   still be running unrestricted on any Gateway that has not yet been
   upgraded.
4. **The policy does not cover native `HTTPRoute` filters.** It governs
   plugins configured through `EdgionPlugins` / `EdgionStreamPlugins`
   only. A filter you deny as, say, `http/RequestRedirect` is still fully
   expressible as a native Gateway API `HTTPRoute` filter — the policy
   does not (and cannot) restrict that path. If you need to prohibit a
   behavior outright, denying it here is not sufficient on its own.
5. **A `block` gate is unconditional.** It rejects a request even when
   that request's per-entry `conditions` (e.g. a time-window condition)
   would have skipped the plugin anyway. If you rely on time-windowed or
   otherwise conditional plugin execution, be aware that policy-blocking
   that plugin type overrides the condition entirely — the chain is
   rejected regardless of whether the plugin would have actually run for
   that particular request.

## Related documentation

- [Controller Configuration — validation.plugin_policy](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/02-config/00-controller-config.md#validationplugin_policy) — full field reference.
- [EdgionPlugins — HTTP Plugins](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/03-resources/13-edgion-plugins.md) — HTTP plugin catalog (`http/` family names).
- [EdgionStreamPlugins — Stream Plugins](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/03-resources/14-edgion-stream-plugins.md) — stream/TLS-route plugin catalog (`stream/` and `tls-route/` family names).
