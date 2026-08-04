// Transport-shape test backends.
//
// Two raw listeners plus one process-global event recorder, added so the Edgion
// end-to-end suite can drive and observe *transport-level request-body shapes*
// through the real gateway: framing selection, termination hygiene, peer resets
// at chosen byte offsets, upstream connection identity and reuse, and
// deliberately malformed response framing.
//
// | Flag                  | Default port | Speaks                                |
// |-----------------------|--------------|---------------------------------------|
// | `--transport-h1-port` | 30030        | raw HTTP/1.1 over a tokio TcpListener |
// | `--transport-h2-port` | 30031        | h2c prior knowledge via `h2::server`  |
//
// Neither is an axum route, deliberately: `axum::serve` cannot express
// half-close, a RST at a chosen offset, connection identity claimed at
// `accept()`, or a response whose framing contradicts itself — and wiring that
// machinery into the shared listener would put it on the path of the ~40
// endpoints five other suites depend on.
//
// The query surface (`/transport/query/{trace_id}`, `/transport/reset`,
// `/transport/dump`) does live on the shared axum router, so cases reach it
// with plain `reqwest`, exactly like `/mirror/query/{trace_id}`.
//
// Control-header vocabulary and event vocabulary are documented in
// `skills/05-testing/references/test-server-capabilities.md`.

pub mod h1;
pub mod h1_wire;
pub mod h2;
pub mod recorder;
pub mod script;
