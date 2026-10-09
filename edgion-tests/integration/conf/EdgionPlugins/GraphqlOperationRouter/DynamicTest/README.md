# GraphQL operation routing hot reload

Uses the shared Controller/Gateway, EdgionPlugins listener and three existing HTTP
test-server listeners. The parent suite provides same-namespace Services and a
weight-zero cross-namespace backend; no Redis dependency or dedicated processes.
Echo endpoints are transport/backend-identity oracles, not GraphQL execution.

Phases prove initial users routing, an invalid unselected mutation mapping,
restoration, removal/restoration of a declared backend, plugin-only switch to the
orders backend, ReferenceGrant addition and subsequent removal. Invalid binding
withdraws only the protected rule: the health sibling remains usable. Every phase
uses bounded traffic convergence through wait_phase. Grant probes select the
weight-zero target explicitly, so weighted fallback cannot satisfy the test.
