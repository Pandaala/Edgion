# D0015 — Stale mTLS Helper Knowledge

- Status: resolved
- Knowledge destination: `skills/04-review/tls/regression-guards/mtls-config-reorder.md`
- Discovered in: B0075

## Evidence

`skills/04-review/tls/regression-guards/mtls-config-reorder.md` still names the removed `apply_secret_to_ssl` helper in two places. B0075 updated the directly owning serving-chain and installer-decision pages, but this unrelated historical residue was outside its approved six-file scope.

## Boundary

Revalidate the mTLS ordering guard against current BoringSSL callback symbols, replace only stale helper ownership statements, and preserve its historical evidence and FIPS/mTLS ordering invariant.

## Resolution

The regression guard now identifies the removed Gateway Secret installer as historical and names the current shared compiled-identity installer. Its fail-close ordering and historical evidence remain intact.
