# D0097 — Align Response-Body Filter Terminal Documentation

- Status: pending
- Severity: medium (architecture guidance drift)
- Discovered in: B0144 response-body-filter trait analysis
- Canonical task: [Align response-body filter terminal documentation](../../../todo/align-response-body-filter-terminal-documentation.md)

The plugin-system and plugin-development guides summarize this phase as only per-body-chunk processing. They omit the synthetic terminal-without-trailers/header-only callback and the exclusion of TerminalBeforeTrailers. Align both guides in a separate cross-document review; no integration change is required.
