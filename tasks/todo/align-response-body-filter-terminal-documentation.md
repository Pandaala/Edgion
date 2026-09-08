# Align Response-Body Filter Terminal Documentation

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | documentation |
| Priority | P2 |

## Requirements

- Align the response-body-filter phase in the Gateway plugin-system and plugin-development guides with the current driver.
- Document data callbacks, terminal-without-trailers/header-only completion, and the exclusion of TerminalBeforeTrailers.
- Keep trailer-aware finalization owned by `UpstreamResponseTrailer` and preserve the streaming-only response boundary.
- Validate links and phase ordering against current code.
