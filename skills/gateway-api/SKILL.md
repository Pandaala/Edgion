---
name: gateway-api
description: Compatibility notes for Edgion's Gateway API handling, especially intentional deviations in TLS certificate selection such as no hostname-less catch-all matching and no cross-port certificate fallback.
---

# Gateway API Compatibility Notes

Use this note when changing Gateway API behavior in Edgion and you need to understand intentional compatibility boundaries.

## TLS Certificate Selection

Edgion intentionally does not support two looser Gateway API-adjacent TLS behaviors:

1. Listener catch-all for TLS certificate selection
2. Cross-port TLS certificate fallback

### What Edgion does

- TLS certificate selection must match the current listener port.
- Listener `hostname` must be present to participate in Gateway TLS certificate matching.
- Listener entries without `hostname` are ignored for dynamic TLS certificate selection.
- If no `(port, sni)` match exists, certificate selection fails.
- Explicit `fallback_sni` remains an Edgion-specific override, not a generic catch-all.

### What Edgion does not do

- No hostname-less catch-all listener certificate selection
- No search across all listener ports for a usable certificate
- No implicit fallback from one listener's certificate to another listener on a different port

## Why catch-all TLS matching is not supported

Gateway API allows a Listener without `hostname`, but that only means the listener matches all hostnames at the routing level. It does not guarantee the configured certificate is valid for every incoming SNI.

For TLS termination, Edgion prefers strict certificate selection rules:

- exact SNI match
- wildcard SNI match
- explicit fallback configuration

This avoids selecting a certificate that has no clear relation to the requested SNI.

## Why cross-port fallback is not supported

Gateway listeners are port-scoped. Allowing a fallback that ignores port breaks listener isolation and can select the wrong certificate when the same hostname is configured differently on multiple ports.

Edgion treats `(port, sni)` as the required lookup key for Gateway TLS.

## EdgionTls binding rule

For `EdgionTls`, controller-side port resolution is required before the resource can enter the matcher.

- If parent refs resolve to listener ports, the TLS resource is admitted into the matcher.
- If no listener port is resolved, the TLS resource is skipped from the matcher.
- This state should be reflected in status instead of being hidden behind a global fallback.

## Implementation guidance

When editing TLS matching:

- keep `EdgionTls` matching port-scoped
- keep Gateway TLS matching port-scoped
- do not reintroduce hostname-less catch-all certificate selection
- do not reintroduce cross-port fallback lookup
- prefer explicit status/reporting over implicit matcher fallback
