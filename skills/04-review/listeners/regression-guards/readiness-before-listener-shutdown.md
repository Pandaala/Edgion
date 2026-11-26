---
name: readiness-before-listener-shutdown
description: Review SIGTERM ordering, endpoint removal and Pod termination budgets.
knowledge_type: regression-guards
---

# Withdraw readiness before listener shutdown

On Unix SIGTERM, the Gateway signal adapter withdraws readiness before forwarding
graceful termination to Pingora. Its process flag stays set until exit, including
with a zero-second withdrawal window. During a positive window, probes remain live
and new business connections can complete. SIGINT and a fatal PROXY policy change
can still request fast shutdown. After propagation, Pingora owns accept-loop exit,
keepalive and active-stream draining. UDP cleanup follows Pingora shutdown.

For `gw-core-01`, the probe previously observed only config readiness and policy
failure. The fork broadcast its shutdown watch before its grace sleep, and the Pod
budget matched that sleep alone. Recheck the Kubernetes ConfigMap and Deployment
together whenever shutdown defaults or probes change. Budget for withdrawal,
both Pingora timeouts, UDP cleanup, exporter shutdown and scheduling margin.
Endpoint propagation time and final cleanup are deployment-dependent.

The isolated `test_http_process_shutdown.py` scenario checks readiness withdrawal,
fresh HTTP connections, idle keepalive closure and active SSE/gRPC disposition.
See the [process lifecycle](../../../01-architecture/02-gateway/01-startup-shutdown.md)
for shutdown ownership across runtimes. All four process cases passed with
work stealing enabled and disabled, including graceful stream completion and
timeout. Separate process checks interrupted a 60-second withdrawal with SIGINT
and a fatal policy update, verifying successful and non-zero exits respectively.
