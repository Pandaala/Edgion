# Condition Reference Hot-Reload Test

This scenario verifies that Controller resolution and Gateway hot reload use the
same nested `SecretMatch` dependency from both `body.conditions` and a Dye rule.
The initial and rotated phases require the current Secret value to enable the
`RequestBodyBuffer` gate and the response Dye header. A late DSL reader proves
whether the shared request-body cache was populated. Stale values skip capture,
so the late reader fails closed without rendering Dye. Deleting the Secret fails
the plugin resource closed, and restoring it re-enables both consumers without
restarting either binary.

The same topology also checks live IpList consumers. `condition-refs-live-gate`
uses an ordinary entry condition and `condition-refs-live-rap` uses positive RAP
membership. Only the IpList changes: initial 418/200, content update 200/403,
delete 500/403, restore 418/200. The scenario preserves the existing Secret phases
and restores both shared resources after success or failure. Each YAML file contains
one resource; no plugin publication is needed for the IpList content phases.
