# MCP HTTPRoute dynamic lifecycle

`McpRouteLifecycle` verifies the WP10 Controller-to-Gateway configuration contract without
sending data-plane or MCP protocol traffic. It uses the shared integration Controller and
Gateway, unique resource names and hostname, bounded phase polling, and cleanup before and after
the scenario.

The lifecycle covers an MCP backend with an invalid `PathPrefix`, MCP-to-A2A and A2A-to-MCP
replacement, repair to an explicit `Exact` path, and backend delete/recreate. Every phase compares
the Controller processed HTTPRoute with the Gateway HTTPRoute config cache. In particular, backend
deletion must clear the MCP-specific invalid index and expose ordinary `BackendNotFound` status;
this proves that eligibility is recomputed from the current backend generation rather than a
persisted marker or tombstone.

MCP request parsing, Origin enforcement, upstream transport, and public listener activation are
covered by later WP10 scenarios.
