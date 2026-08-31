# D0030 — Retain EdgionPlugins Last-known-good Only for the Same Resource Identity

## Meta
- Status: needs-discussion
- Severity: high
- Discovered in: B0062, HTTP plugin framework audit
- Category: correctness-security
- Canonical task: [`tasks/todo/retain-edgionplugins-last-known-good-resource-identity/`](../../../todo/retain-edgionplugins-last-known-good-resource-identity/retain-edgionplugins-last-known-good-resource-identity.md)

## Evidence
Partial config events distinguish a same-object update from delete-to-create coalescing, so B0062 can make adds replace unconditionally while updates retain valid DSL/WAF last-known-good runtime. A reconnect or relist full snapshot supplies only namespace/name keys. `PluginStore::replace_all` therefore cannot distinguish an invalid update from a different resource recreated at the same key and may retain runtime from a deleted generation.

## Why deferred
Correct retention needs one cross-source generation identity contract for Kubernetes, FileSystem, and etcd inputs and must preserve Controller-to-Gateway conf-sync semantics. The existing TLS identity task owns TLS certificate/listener/host side effects; it is a design precedent, not ownership of EdgionPlugins compilation, stage, and runtime state. A plugin-local Kubernetes UID shortcut would leave non-Kubernetes sources and reconnect parity undefined.

## Integration-test impact
Existing integration tests remain frozen. D0031 records the source-to-compression/full-snapshot scenario required after this identity design is implemented.
