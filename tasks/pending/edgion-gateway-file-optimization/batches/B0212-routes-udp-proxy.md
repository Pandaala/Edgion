# B0212 — UDP Proxy

## Scope

- Pipeline state: analysis-in-progress; third queued candidate, not the active mutation batch.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/udp/edgion_udp.rs`.
- Baseline: `f48fca38`.
- Analyst: `b0206_tls_passthrough_analysis`, acting only as the B0212 analyst.
- Investigate session/socket and route ownership, bounded lifecycle, plugins,
  endpoint safety and canonical traffic observability through actual callers.
- No implementation authorized; existing integration files and user-owned paths remain protected.
