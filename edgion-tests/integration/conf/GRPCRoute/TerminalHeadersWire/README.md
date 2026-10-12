# Terminal gRPC header preparation wire regressions

Run the shared-stack suite with:

```bash
./edgion-tests/integration/scripts/integration/run_integration.sh --no-prepare -r GRPCRoute -i TerminalHeadersWire
```

Build the current canonical artifacts with `integration/scripts/utils/prepare.sh`
before using `--no-prepare`. All six cases reuse the same Gateway, Controller,
ExtProc processor, auth service and gRPC backend.

The request exact and response exact cases use opt-in ExtProc fixture modes,
returning status 8 with an absent message. Queued AddIfAbsent/OverwriteIfExists
operations distinguish adapter headers from the framework Server default. HTTP
listeners omit HSTS, so its OverwriteIfExists operation must remain a no-op on
all three paths. Ordered AddIfAbsent/OverwriteIfExists operations on x-terminal-order
independently prove queue ordering.
The mapped case uses the auth fixture's fixed 401 denial and ForwardAuth's actual
self-write, yielding status 16; its DSL queue exercises ordinary Set/Append/Remove
operations. It does not claim coverage of lazy conditional operations on that path.
Every case preserves canonical request identities, queue order and repeated cookies,
and verifies upstream counts through the access log.

Native H2 cases use a bounded transparent frame scanner beside the real h2 decoder
and require a single initial HEADERS with END_STREAM and zero DATA/RST frames.
The scanner retains only type/flags/stream metadata, handles fragmented reads,
counts all incoming bytes/frames and rejects incomplete or failed observations.
All driver tasks are cancelled on exit or deadline. The web cases require exactly
one complete 0x80 trailer frame and no initial conflicting status/message fields.
They verify decoded web framing, not HTTP transfer-chunk boundaries.

These cases supplement ARCH-003's focused tests. They do not inject forbidden
framing mutations through the queue, simulate delivery failure or duplicate
claims, or test hot reload; those boundaries retain unit/static coverage.
