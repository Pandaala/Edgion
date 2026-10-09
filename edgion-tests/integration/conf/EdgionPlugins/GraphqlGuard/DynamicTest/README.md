# GraphQL guard hot reload

Shared Controller/Gateway topology; no Redis or dedicated process. Phases verify initial bounds, invalid enabled replacement (fixed 500 and Accepted=False), recovery and stricter alias restriction. wait_phase supplies bounded traffic/status convergence. Echo proves original request preservation, not backend GraphQL execution.

Final root-policy phase rejects ping and its alias with403 while allowing pong; Controller acceptance stays True.

Calibration phases extend the existing sequence: measure -> rejected unknown-mode write (400, active measure generation retained) -> invalid maxCost (Accepted=False/fixed500) -> measure recovery -> enforce. Above-threshold forwarding verifies original bytes, completed cost/header/log and exactly one upstream; introspection/root permissions and malformed/schema failures remain rejected.

Standard SDL metadata phases accept deprecated fields and custom-scalar specifiedBy, reject null directive arguments with Accepted=False/fixed500, then recover without changing cost/debit semantics.
