# DSL query-ID integration fixtures

The shared topology loads LinkSys/Redis, the selector-owned ResponseCache Service,
and Dsl fixtures. Static Dsl appends one sequential contract case using fixed
id dsl-query-lookup-static; DslQueryLookupDynamic owns its separate namespace and
id dsl-query-lookup-dynamic. Every request carries x-response-cache-fixture at
entry, including negative publication/auth/method probes; trusted DSL overwrites
the same literal. This tag is instrumentation, never tenant authority.

The origin ingress counter counts tagged arrivals before method/path dispatch.
/response-cache/observations/{id} and POST reset are direct test-origin-only APIs.
GraphQL replies expose bounded original-byte digest/length, fixed method/path,
credential-absence boolean and backend port without query text or DOM rounding.

Run the Kubernetes runner with -r EdgionPlugins -i DslQueryLookupDynamic.
The dynamic helper stages this Kubernetes fixture tree into the shared test client.
No second topology or server is created. Invalid updates stay in updates/invalid
and are excluded from preload. Invalid updates retain the good prior DSL; a
fresh invalid owner fails closed. Scenario polling uses wait_phase and cleans
owned routes first, then known Redis keys and owned plugins/Secrets on all exits.
Shared Redis credentials, LinkSys, Service and processes are never deleted.
