# B0286 — HTTP ExtProc Plugin

- Pipeline state: analysis-in-progress; read-only candidate.
- Active anchor: `edgion-gateway`; baseline `6bf48dcf`.
- Target: `edgion-gateway/src/plugins/http/ext_proc/plugin.rs`.
- Analyst: `b0286_extproc_plugin_analysis`; full plugin-owned lifecycle/value and coverage analysis assigned. Prior B0282 supporting edits did not establish full coverage.
- B0284 owns the serial commit lane; B0285 query correction is queued ahead and may adapt one test expectation here. Refresh this candidate against intervening commits before promotion.
- No edits authorized; existing integrations and user handoff/WAF paths protected. Existing D0148-D0151 and queued query/opaque-header findings must not be duplicated.
