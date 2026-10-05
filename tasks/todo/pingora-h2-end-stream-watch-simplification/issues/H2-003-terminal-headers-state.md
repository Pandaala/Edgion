# H2-003: Replace DATA-Dependent Terminal HEADERS Heuristic

## Severity

P1 response-integrity and compatibility risk.

## Evidence

The current watcher records terminal HEADERS only after nonzero response DATA.
It therefore misses illegal zero-DATA trailers, while treating valid empty
trailers following DATA as suspicious.

## Required change

- Record terminal HEADERS independently of delivered DATA count.
- Keep validated initial-header EOS distinct from terminal trailer HEADERS.
- Use the marker only to fail closed when h2 exposes no validated trailer event;
  do not use raw frame shape to decide trailer validity.
- Preserve valid empty and nonempty trailers exactly once.

## Acceptance tests

- Zero-DATA invalid trailers and DATA-plus-valid-empty trailers.
- Header-only final response, with informational prefixes.
- Async, poll, and direct-trailer paths, each with a same-burst reset.

## Related files

- `pingora-core/src/protocols/http/v2/end_stream_watch.rs`
- `pingora-core/src/protocols/http/v2/client.rs`
