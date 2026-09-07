---
name: review-websocket
description: Review knowledge for websocket, grouped by decision type.
---

# websocket review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [ws-01 / ws-02 / ws-03 — WebSocket handshake not validated at the gateway](intentional-design/handshake-not-validated.md)
- [ws-05 — Backend `appProtocol=ws/wss` forces HTTP/1.1; no RFC 8441 (WS-over-H2)](intentional-design/no-rfc8441-h2-extended-connect.md)
- [ws-04 — WebSocket `Origin` not validated at the gateway](intentional-design/origin-not-validated.md)
