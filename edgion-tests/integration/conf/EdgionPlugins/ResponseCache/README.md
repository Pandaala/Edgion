# ResponseCache integration contract

The shared EdgionPlugins Gateway listens on port 31180. This fixture uses
`response-cache.example.com`, one HTTPRoute rule shared by GET `/cache` and
POST `/graphql`, and the existing test-server Service on port 30001.

KeyAuth removes the single `x-response-cache-key` credential before upstream
forwarding. GraphqlGuard inspects POST requests before ResponseCache; RequestId
runs on every request, including replay. The test credential is
`test-response-cache-key` and carries no consumer fields.

The static contract executes sequentially with the fixed
`response-cache-static` ingress tag. It observes actual origin attempts through
the bounded test-server observation interface. A negotiated direct H1 trailer
probe finishes before resetting the owned count; Gateway declared-trailer bypass
is checked separately. Reqwest text collection does not prove terminal trailers.

DynamicTest uses distinct resource names, hostname and the
`response-cache-dynamic` ingress tag. Phases disable and reenable caching, add a
valid response-body BandwidthLimit companion, replace the existing EndpointSlice
target with port 30002, and perform real HTTPRoute deletion. Updates must become
Accepted=True before their behavior is assessed. Invalid updates retain the
predecessor and are not an invalidation proof.

Run through the standard integration runner on its one owned shared topology.
These fixture definitions do not establish a passing live result by themselves.

Each dynamic transition owns one bounded miss-to-hit receipt. Its fixed ingress
counter remains monotonic across transitions; only the receipt resets. The finite
dynamic origin includes its captured ingress ordinal in at most 128 response
bytes. Replay must preserve that exact new ordinal and add no origin attempt,
so an old warm entry cannot substitute for a newly captured response. Repeated
reenabled phases have independent receipts. Resource receipt is a readiness
precondition, not proof that the publication epoch has stopped changing.
