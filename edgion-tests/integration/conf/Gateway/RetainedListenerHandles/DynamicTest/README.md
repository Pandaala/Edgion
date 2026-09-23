# Retained listener route handles

This dynamic scenario runs in the shared Controller/Gateway topology. The
three dedicated ports are bound at startup. HTTP and gRPC requests and new TCP
connections pass through the original listener proxies before and after the
Gateway and Routes are deleted. The initial probes must reach the HTTP, gRPC,
and TCP test backends; after deletion, the original listeners must not reach
them. Recreating the same Gateway and Routes must populate new registry
managers while the original listeners still fail closed. The store-stats
comparison checks that the new registry entries contain routes, rather than
mistaking a newly created empty manager for a cleared old handle.

The driver removes Gateway ownership first and then the Routes. Controller
dependency requeues and Gateway configuration streams can interleave, so a
wire-level result cannot identify which of the full stale sweep or partial
orphan sweep published the empty snapshot. The HTTP and gRPC manager unit tests
and the shared L4 helper tests inject the active-listener set and retain the
exact old `Arc`; they assert both sweep branches clear that `Arc` before
dropping the registry entry. This scenario checks the cross-process routing
consequence of those invariants.

TLS and UDP use the same retained per-listener manager pattern. TLS adds
certificate and SNI selection, which are independent of the manager-clearing
ordering tested here; its unit tests cover the clear pass. UDP sessions pin
their selected upstream, so deletion is expected to affect new sessions,
not frames on an established session. Neither requires a separate wire case
for this listener-handle regression.
