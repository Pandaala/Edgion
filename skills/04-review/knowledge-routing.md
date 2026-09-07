---
name: review-knowledge-routing
description: Classify and maintain Edgion review knowledge by owning function and decision type.
---

# Review knowledge routing and maintenance

## Ownership and authority

Use `04-review/<function>/<knowledge-type>/<semantic-slug>.md`. Function is the
mechanism that owns the behavior, not the finding's severity, review batch, or author.
Cross-cutting topics such as security, concurrency, and observability own shared
mechanisms; a feature-specific consequence can link to them without copying the rule.
Search the function index and the whole tree by symbol, behavior, and finding ID before
choosing a destination. Create a function directory only when no existing owner fits.

The authoritative graded rules remain in `../00-design-rules.md`, architecture, and
coding documents. Classification does not create a new MUST, override those rules, or
ratify a known defect. A review decision applies only within its recorded scope and
assumptions. Revalidate current code before suppressing a new finding.

## Knowledge types

| Directory | Admission criteria | Required decision content |
|---|---|---|
| `mandatory-design` | An established required invariant, with an explicit project or protocol contract. | Scope, canonical authority, invariant, violation consequence, relevant checks. Do not promote a preference or old fix solely because its text says "must". |
| `intentional-design` | A consciously accepted choice or bounded tradeoff; alternatives were considered. | Chosen behavior, acceptance evidence, benefit, cost/risk, rejected alternatives, concrete re-evaluation triggers. A missing feature or unfixed bug is not accepted merely because it is old. |
| `compatibility-constraints` | A specific dependency API, external protocol/peer, or supported deployment constraint forces the shape or requires an interop escape hatch. | Exact constraint and version/surface, evidence, affected scope, mitigation, removal/revisit condition. Similar behavior in another gateway is not proof of necessity. |
| `regression-guards` | A verified correction or established regression boundary. | Trigger, correct behavior, root cause/fix, source and test evidence, limits of the verification. This does not elevate every implementation detail into a design mandate. |
| `review-evidence` | A disproved premise, review methodology, or historical evidence insufficient to establish a current rule. | Original claim, decisive counter-evidence, scope, uncertainty, re-evaluation conditions. No blanket "do not report" instruction. |
| `superseded` | The old decision or owning implementation has been replaced or removed. | Clear inactive notice, what changed, successor rule/source when available, residual valid constraints. Never apply the obsolete conclusion as a current gate. |

These are knowledge types, not finding dispositions. A fixed finding can establish a
mandatory invariant, correct an existing tradeoff page, or retire old guidance. An
accepted RFC divergence may be intentional or externally constrained; classify by its
reason, not by the word RFC. A deferred implementation belongs in `tasks/`, with a
short task pointer here only when it explains a lasting decision. Known unsupported
Gateway rustls behavior stays outside current defect scope.

Edgion is pre-release and has no historical compatibility obligation. The compatibility
category cannot authorize legacy aliases, deprecated fields, or migration shims.

## Update procedure

1. Read the root rules, knowledge index, function index, and directly related pages.
2. Trace the current behavior and distinguish historical resolution evidence from a
   fresh verification. Do not invent dates, tests, acceptance, or dependency guarantees.
3. Prefer rewriting the existing page for the same mechanism. Consolidate repeated
   conclusions and retain useful source IDs, counter-evidence, verification, and caveats.
   Several findings may support one rule; one finding may update multiple independent
   mechanisms. Do not create one archive file per finding by default.
4. Pick one primary knowledge type. If distinct mechanisms need different types, split
   their reusable rules and link them. If a decision changes type, move its page and fix
   every inbound reference. Preserve obsolete evidence in `superseded` only if useful;
   mark obsolete paragraphs inside an otherwise current page explicitly as history.
5. Follow `_template.md` for new or substantially rewritten pages. Existing reference
   shapes may remain if their conclusion, scope, evidence, and limitations are clear.
   All pages carry `knowledge_type` matching the directory; other original metadata
   is retained. A migration is not a fresh code audit or a new acceptance date.
6. Update the owning function's `SKILL.md` with a short categorized link. Update the
   root index only for new functions. Indices contain navigation, never copied verdicts.
7. Before deleting an open finding, confirm its reusable conclusion and stable ID are
   discoverable in the maintained page(s), required independent review/checks passed,
   and unresolved work is still tracked. Report the exact destination and knowledge type.

Do not create a global closed-findings ledger, chronological resolution log, per-module
`closed-findings` archive, or an index containing full case summaries. The searchable
knowledge pages are the destination. A second page for the same rule is not an archive
requirement. Concurrent writers to the same semantic page must serialize their updates;
separate finding IDs do not make overlapping edits independent.

## Migration evidence

The September 2026 organization change preserves topic-page bodies and moves historical
resolution IDs/dates into their owning pages. Legacy rows with no detailed home retain
historical evidence there, explicitly without claiming a new code audit. Old source
locations and original verification claims must be revalidated before reuse. No runtime
behavior or previously accepted product boundary is changed by this reorganization.
