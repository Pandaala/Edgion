# EdgionBackend Lifecycle Dynamic Test

Scenario: `AiBackendLifecycle`.

This self-cleaning scenario exercises two hot-reload state machines through the
Controller Admin API while observing Controller status, Gateway config-client
publication, and representative AI traffic:

1. A cross-namespace HTTPRoute to EdgionBackend reference and a cross-namespace
   EdgionBackend to Secret reference start denied, become allowed after their
   respective ReferenceGrants are added, fail closed when each grant is deleted,
   and recover when it is restored. Deleting and recreating the backend also
   proves the route dependency requeue.
2. An all-AI two-target EdgionBackendTrafficPolicy publishes with the internal
   `resolvedTargetClass: Ai` marker. Deleting either target withdraws the whole
   policy from the Gateway and recreating it restores publication. Invalid
   AI-field and mixed Service/EdgionBackend policies are rejected without partial
   Gateway publication. A conflicting sibling is promoted after the winner is
   deleted.

The scenario reuses the `EdgionBackend/SingleProvider` Gateway and local mock AI
upstream, but all resources created here use dedicated names and are removed at
the end even when an assertion fails.
