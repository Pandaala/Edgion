# Condition Reference Hot-Reload Test

This scenario verifies that Controller resolution and Gateway hot reload use the
same nested `SecretMatch` dependency from both `body.conditions` and a Dye rule.
The initial and rotated phases require the current Secret value to enable the
`RequestBodyBuffer` gate and the response Dye header. A late DSL reader proves
whether the shared request-body cache was populated. Stale values skip capture,
so the late reader fails closed without rendering Dye. Deleting the Secret fails
the plugin resource closed, and restoring it re-enables both consumers without
restarting either binary.
