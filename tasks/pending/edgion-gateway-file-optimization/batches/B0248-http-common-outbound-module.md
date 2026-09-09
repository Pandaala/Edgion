# B0248 — HTTP Common Outbound Module

## Scope

- Pipeline state: examined-no-change; independent review and verification complete, awaiting local record commit.
- Active anchor: `edgion-gateway`.
- Baseline HEAD: `a9236e7579d09198c4c5315929103cfcfef0d9da`.
- Target: `edgion-gateway/src/plugins/http/common/outbound/mod.rs`.
- Target blob: `37f48cab88568a0031341f0b310701c4a2ab6756`.
- Read scope includes the complete outbound module and all consumers of its facade.
- Frozen write scope is not yet authorized; analysis is read-only.
- Existing `edgion-tests/**`, `tasks/handoff/`, `tasks/waf-todo/`, and `waf-architecture-and-multi-pattern-matching.md` remain protected.

## Analysis Outcome

- Decision: examined-no-change.
- The seven-line facade declares six distinct, live ownership boundaries. Workspace-wide searches found no external crate consumer, but narrowing only these modules to `pub(crate)` would provide rustdoc tidiness rather than runtime, correctness or maintenance value; the repository has explicitly rejected broad `unreachable_pub` churn for the same reason.
- Flat re-exports or module merges would blur call execution, client lifecycle, DNS policy, pinning, request compilation and target-selection ownership. Every child has production or directly related test consumers.
- No runtime cost, duplicate mechanism, security bypass, deferred finding or integration gap is attributable to this file. Current HEAD/blob remain `a9236e75` / `37f48cab88568a0031341f0b310701c4a2ab6756`.
- Refresh at `98b88473`: B0247 changed only TTL publication/cache arithmetic inside the egress child and common core. The target blob and all six live facade responsibilities/consumer boundaries remain unchanged; the no-change decision still holds.

## Outcome

- Retained the minimal six-child facade unchanged. Each module owns a distinct live boundary: call execution, managed-client lifecycle, Gateway egress adaptation, pinned resolution, request compilation, or target selection.
- Independent review returned LGTM with no Must Fix, Should Fix or deferred findings. It confirmed all children have real consumers, the crate is unpublished, and narrowing visibility would only apply an explicitly rejected `unreachable_pub` tidiness policy piecemeal.
- B0247 did not change declarations, exports, consumers or ownership. Target blob remains `37f48cab88568a0031341f0b310701c4a2ab6756` at refresh HEAD `98b88473`.
- No source or integration change and no new issue.
- Verification: reviewer Gateway all-target check, Gateway no-tracing, English-only, diff and integration freeze passed; root workspace format/check/Clippy plus agent-doc, SSA, metrics, Pingora, Gateway no-tracing, English-only and freeze gates passed.
- Commit: pending; never push.
