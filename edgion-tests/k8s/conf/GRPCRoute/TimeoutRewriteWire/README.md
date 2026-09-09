# gRPC timeout rewrite wire coverage

This Kubernetes and local-stack suite sends pure gRPC and gRPC-Web requests through a fixed 300 ms request plugin to the raw H2 recorder. The backend records only the parsed numeric `grpc-timeout` value and unit. Its retry script resets the first two requests for one trace with `REFUSED_STREAM`, then emits a valid unary gRPC response, making three upstream attempts deterministic without attributing their count to route retry configuration.

The local fixture configures retry backoff and verifies its timing. The Kubernetes fixture uses the installed GRPCRoute schema, which has no retry-backoff field, so it verifies exactly three transport attempts and strictly decreasing timeout values without asserting a minimum gap between attempts.

The zero-remaining `1n` race remains covered by the focused formatter/upstream-filter unit tests. Scheduling an integration request to expire in the narrow interval between peer selection and the upstream request filter is not deterministic.
