# UDPRoute StreamPlugins integration test scenario

- **Type**: follow-up (test coverage)
- **Origin**: udp-F012 fix (UDPRoute `edgion.io/edgion-stream-plugins` now wired and enforced **per session, at session creation**, fail-close — the F-016 fast-path skips the pipeline for established sessions). See the `udp-F012` entry in `skills/04-review/SKILL.md` and `skills/04-review/cpu-memory/cases/udp-established-session-route-pipeline-per-packet.md`.
- **Status**: open

## Why

The udp-F012 code fix added UDP stream-plugin enforcement at three layers
(`src/types/resources/udp_route.rs`, `src/core/gateway/routes/udp/conf_handler_impl.rs`,
`src/core/gateway/routes/udp/edgion_udp.rs`). The enforcement helper
`check_stream_plugins` already has unit coverage in
`src/core/gateway/routes/tcp/edgion_tcp.rs`, but there is no end-to-end integration
scenario exercising the UDP path specifically.

## Scope

Mirror the existing TCPRoute StreamPlugins integration scenario for UDP:

- Conf YAMLs under `examples/test/conf/UDPRoute/StreamPlugins/`:
  - `EdgionStreamPlugins` allow-localhost (`allowList: [127.0.0.0/8]`, defaultAction deny)
  - `EdgionStreamPlugins` deny-all
  - `Gateway` with UDP listeners (allow / deny / fail-close ports)
  - `UDPRoute` allow-localhost, `UDPRoute` deny-all, `UDPRoute` fail-close (references a missing ESP)
  - `Service` / `EndpointSlice` for a UDP echo backend
- Rust suite under `examples/code/client/suites/udp_route/stream_plugins/`
  (model on `examples/code/client/suites/tcp_route/stream_plugins/stream_plugins.rs`):
  - allow-localhost echo succeeds
  - deny-all drops packets (no echo)
  - fail-close: UDPRoute referencing a non-existent ESP must drop (not silently admit)
  - short-name annotation resolution (namespace inferred from the route)
- Register the suite in `examples/code/client/suites/mod.rs`, add ports to
  `examples/test/conf/ports.json` (key `UDPRoute/StreamPlugins`), and any
  `TestContext` UDP-addr helpers needed (a UDP echo backend already exists in
  `examples/code/server/test_server.rs`).

## Reference

- Code pattern: `examples/code/client/suites/tcp_route/stream_plugins/stream_plugins.rs`
- Conf pattern: `examples/test/conf/TCPRoute/StreamPlugins/`
- UDP basic scenario (for backend/echo wiring): `examples/test/conf/UDPRoute/Basic/`
- Test guide: `skills/05-testing/01-integration-testing.md`
