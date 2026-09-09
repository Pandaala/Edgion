# B0199 — TCP Proxy

## Scope

- Pipeline state: analysis-in-progress; queued, not the active mutation batch.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/tcp/edgion_tcp.rs`.
- Baseline: `21ca0728`.
- Analyst: `b0199_tcp_proxy_analysis`.
- Investigate accept/connect/relay lifecycle, IP trust and egress boundaries,
  plugin dispatch, timeout/health/accounting, and canonical observability.
- Existing integration files and user-owned paths remain protected. No
  implementation is authorized yet.
