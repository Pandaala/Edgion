# Gateway API v1.6.2 dynamic TCP and UDP listeners

## Current state

TCPRoute/v1 and UDPRoute/v1 forward traffic on listener ports bound at Gateway
startup. The v1.6.2 `GATEWAY-TCP` and `GATEWAY-UDP` profiles create listener
ports after startup; those ports are not bound, and both profiles timed out.
The limitation is documented in
[`01-not-implemented.md`](../../skills/08-kubernetes/01-gateway-api/01-not-implemented.md).
Edgion does not advertise `TCPRoute` or `UDPRoute` in
`GatewayClass.status.supportedFeatures`.

## Follow-up

1. Design and implement a way to bind and retire TCP and UDP listener ports as
   Gateway resources change, including Controller-to-Gateway sync behavior.
2. Verify route attachment and real traffic on ports added after Gateway
   startup, and check listener removal and port reuse.
3. Run the complete v1.6.2 `GATEWAY-TCP` and `GATEWAY-UDP` profiles and save
   passing reports before advertising their supported features.
