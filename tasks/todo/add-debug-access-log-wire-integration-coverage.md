# Add Debug Access Log Wire Integration Coverage

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | testing |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Add standalone cross-process coverage for the client-visible debug access-log snapshot without changing the campaign's frozen existing integration tests.

## Requirements

- Start Replace and Wrap cases with separate Gateway process configurations because the sensitivity mode is startup-global, not resource-hot-reloaded.
- Under Replace, assert exactly one parseable `X-Debug-Access-Log` value with sensitive request fields replaced and `ext` plus `waf` absent.
- Under Wrap, assert the response header is absent, status/body behavior is unchanged, and the terminal canonical access log contains `wrap mode:header suppressed`.
- Exercise EdgionPlugins add, disable, remove, and re-enable or replacement so header presence follows the published plugin generation.
- Use new standalone scenarios or later-approved minimal registration and fixture wiring; do not modify existing integration coverage.
- Preserve canonical access-log ownership, bounded diagnostics, response streaming, and sensitive-data isolation.

## Acceptance criteria

- Replace and Wrap wire behavior is proven from Controller publication through the Gateway response.
- Mode cases cannot race or share a process-global sensitivity configuration.
- Hot reload proves the response header follows the active EdgionPlugins generation without restarting for resource changes.
- `ext` and `waf` never appear in the non-terminal debug snapshot.
- Existing integration-test files remain unchanged unless a later task receives explicit authorization to alter shared registration or fixture wiring.
