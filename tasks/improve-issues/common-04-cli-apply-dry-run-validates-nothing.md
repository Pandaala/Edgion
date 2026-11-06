# common-04: `edgion-cli apply --dry-run` validates nothing although the Controller exposes `/api/v1/validate/{kind}`

- **Category**: design-error
- **Severity**: P2 (worth doing)
- **Scope**: local
- **Location**: `edgion-cli/src/cli/commands/apply.rs:148-163` (`apply_doc` dry-run branch), `edgion-cli/src/cli/mod.rs:57-62` (`Apply { dry_run }`), `edgion-controller/src/api/validate_handlers.rs` (`POST /api/v1/validate/{kind}`, routed at `edgion-controller/src/api/mod.rs:505`); related `edgion-cli/src/cli/commands/get.rs:143`, `edgion-cli/src/cli/client.rs:60,82` (`_socket`)

## Summary
`apply --dry-run` only parses `kind`/`metadata.name` locally and prints "Would apply …". It never
contacts the Controller, so schema errors, unknown fields, plugin/DSL/WAF compile errors and LinkSys
validation failures are not surfaced. The Controller already has a purpose-built, read-only endpoint
(`POST /api/v1/validate/{kind}`) returning `schemaErrors`, `unknownFields` and `validationErrors` — the
CLI does not use it anywhere. Because the Controller is "warn-not-reject" for schema/config-key
mismatches, an operator who relies on `--dry-run` as a pre-flight check gets a false green.

## Current behavior (evidence)
- `apply.rs:148-163`: `extract_resource_metadata(content)` → `if dry_run { println!("Would apply …");
  return Ok(()); }`. No network call, no validation.
- `validate_handlers.rs` module doc: "a synchronous, read-only verdict on a submitted resource
  document, so an operator who wants blocking can build it themselves (… a CI check, a GitOps pre-merge
  step)"; it runs the same metadata extraction, Gateway API schema check, typed parse, CRD-schema
  validation, unknown-field walk and EdgionPlugins/LinkSys/request-transform business validation as the
  write path.
- `grep -rn validate edgion-cli/src` finds no call to `/api/v1/validate`.
- Related CLI correctness smells in the same flow: `get.rs:143` tells the user to "Run
  'edgion-cli server-info'", a command that does not exist (`Commands` has no such variant);
  `--socket` is accepted globally but ignored (`EdgionClient::new(.., _socket)`), so a user who passes it
  silently talks TCP to `localhost`.

## Industry comparison
- `kubectl apply --dry-run=server` runs full admission and returns the would-be object.
- Kong `deck gateway validate --online` / `deck file validate`, `istioctl analyze` and
  `apisix-ingress` admission all give an authoritative pre-flight verdict from the same validator the
  control plane uses.

## Impact
CI/GitOps pipelines using `edgion-cli apply --dry-run` (the natural choice) accept invalid manifests
that later land as `Accepted=False` or silently-ignored fields. The one validator built for this exact
job is unreachable from the official CLI.

## Suggested direction
- Make `--dry-run` (or `--dry-run=server`, keeping a `=client` mode for offline parse only) POST each
  document to `/api/v1/validate/{kind}` and print `schemaErrors` / `validationErrors` /
  `unknownFields`; exit non-zero on any error (and optionally on unknown fields with `--strict`).
- Add `edgion-cli validate -f <file|dir>` as an alias for CI use.
- Fix the dangling `server-info` hint (add the command — the endpoint exists — or drop the hint) and
  either implement `--socket` or remove the flag.

## Notes
No review page under `skills/04-review/` covers CLI dry-run semantics (searched `dry-run`, `validate`,
`edgion-cli`). The validate endpoint's own doc states the warn-not-reject policy that makes this gap
consequential. No overlapping task under `tasks/`.

## Verification (independent)

- **Verdict**: CONFIRMED
- **Severity**: P2
- `edgion-cli/src/cli/commands/apply.rs:160-163`: `if dry_run { println!("Would apply ..."); return Ok(()); }` after local metadata parse only.
- `grep -rn validate edgion-cli/src` finds only cert-name/wasm helpers; no call to `/api/v1/validate/{kind}`.
- `edgion-cli/src/cli/commands/get.rs:143` tells users to run `edgion-cli server-info`, but `Commands` (`edgion-cli/src/cli/mod.rs:76-167`) has no such variant.
- `edgion-cli/src/cli/client.rs:86`: `EdgionClient::new(.., _socket)` ignores the flag.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P2
- `edgion-cli/src/cli/commands/apply.rs`, `get.rs`, `client.rs` and `cli/mod.rs` are unchanged; dry-run still returns after a local metadata parse.
- `validate_handlers.rs` changed by one line only; the CLI still never calls `/api/v1/validate/{kind}`.
