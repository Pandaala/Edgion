# Historical Self-Built WAF Proposal

Status: superseded; not an implementation backlog.

The native request WAF is implemented. Current contracts are in the
[WAF knowledge index](../../skills/04-review/waf/SKILL.md) and the pinned
[supported CRS package](../../config/waf/crs-4.25.1-supported-v1/README.md).
The frozen proposal below does not describe current configuration, distribution,
or acceptance. In particular, embedded CRS and full response-body support are
not current requirements. Remaining work is tracked in separate short tasks.

> Spec (final, approved 2026-07-12): `docs/superpowers/specs/2026-07-12-waf-design.md`
> Supersedes the exploratory note `waf-integration-modsec-coraza.md` (repo root; can be deleted).

## What was decided

Full design is complete and frozen; no implementation planned for now. Key decisions:

- **Engine: self-built** (`edgion-waf` leaf crate, symmetric with `edgion-dsl`).
  zentinel-modsec (too young: 0.1.x, single maintainer, no CRS-corpus evidence) and
  Coraza serve as reference implementations and differential-test oracles, not deps.
- **All 5 ModSec phases**, implemented in stages (request side first, response side second).
- **Request body: buffered verdict-before-forward** ("zero bytes upstream before verdict"),
  via a new `read_request_body_buffered(limit)` session API + ctx replay in
  `pg_request_body_filter`. No new plugin phase needed.
- **Rules: EdgionPlugins base + new EdgionConfigData variant `WafRuleSet`**; fixed
  5-segment assembly pipeline; structured CRS tuning knobs + SecLang escape hatch;
  fail-to-base with embedded CRS as the always-present protection floor.
- **CRS distribution: embedded in the binary** (2 most recent versions, explicit
  `crs.version`, rollback = one field flip) + EdgionConfigData hot patches for
  emergency rules. No auto-fetching (explicit non-goal).
- **Acceptance is quantified**: official CRS go-ftw corpus >= 99% pass + differential
  CI against Coraza; known-divergence file for the honest boundary.

## When picking this up

1. Read the spec, then run `superpowers:writing-plans` with the spec as input to
   produce the M1 implementation plan (spec section 9 has the M1-M4 roadmap and exit
   criteria).
2. First technical risk to validate (spec section 10): pingora downstream body
   pre-read in `request_filter` + injected replay in the forward loop, against the
   pinned pingora version. Fallback is documented (hold-and-release).
3. Related pending item:
   `tasks/pending/request-body-capability-followups/01-json-schema-validation.md` — the body
   pre-read/replay infrastructure built for the WAF likely unblocks it (and
   `hmac_auth` body validation); coordinate to avoid duplicate infrastructure.

## Priority

Not urgent. No implementation scheduled; design is ready for paint-by-numbers
execution whenever WAF becomes a priority.
