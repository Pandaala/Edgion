---
name: algorithms
description: Algorithm catalog for Edgion — the selection rationale, exact math, complexity/concurrency contract, and prior-art comparison for every non-trivial algorithm in the codebase. Read before changing a scoring function, cache eviction policy, matcher, or hashing scheme, and before proposing a replacement algorithm.
---

# 12 Algorithms

> Per-algorithm decision records. Each entry states **what we run today**, **why that
> algorithm and not the alternatives**, and **what would have to be true to replace it**.
> This is the durable home for algorithm comparisons so that the same "should we switch to
> X?" question is answered from evidence instead of re-derived every time.

## Scope — what belongs here

An entry belongs in this chapter when the code embodies a **named algorithm with alternatives**
— something where a reasonable engineer could ask "why this one?" and a wrong answer changes
latency, distribution, memory, or correctness under concurrency.

| Belongs here | Belongs elsewhere |
|---|---|
| The scoring math, decay constants, and their derivation | How the LB is wired into request handling → `01-architecture/02-gateway/` |
| Why P2C instead of a min-heap; why TinyUFO instead of LRU | The YAML field that selects a policy → `02-features/` |
| Complexity, concurrency contract, saturation/overflow behavior | Coding style rules → `03-coding/` |
| Benchmark results and the adopt/reject decision they drove | A specific review finding's verdict → `04-review/` |

**Boundary with `04-review/`**: a review finding records *"this reported issue is/isn't a
defect"*. An algorithms entry records *"this is the algorithm, here is its contract, here is
the prior art"*. When a review closes by accepting an algorithmic tradeoff, the review file
stays the canonical verdict and links here for the math; do not duplicate the rationale.

## Catalog

Ordered by subsystem. "Doc" links only where a written entry exists; the rest are tracked
placeholders — write the entry when you next touch that code.

### Load balancing

| Algorithm | Source | Doc |
|---|---|---|
| Power of Two Choices (P2C), distinct-index sampling | `edgion-gateway/src/lb/selection/p2c.rs` | see `04-review/lb/p2c-choice-count-fixed-at-two.md` |
| Time-decayed Peak-EWMA latency scoring | `edgion-gateway/src/lb/ewma/score.rs`, `edgion-gateway/src/lb/backend.rs` | [lb-peak-ewma.md](lb-peak-ewma.md) |
| Provider overload penalty (429, or 503 + `Retry-After`, as a synthetic latency sample) | peak-only store shipped (`score.rs::observe_peak_only`, used by the 5xx path); no 429 caller written | [lb-overload-penalty.md](lb-overload-penalty.md) |
| Consistent hashing ring (session affinity) | `edgion-gateway/src/lb/selection/consistent_hash.rs` | not yet written |
| Weighted round robin | `edgion-gateway/src/lb/selection/round_robin.rs` | not yet written |

### Caching and eviction

| Algorithm | Source | Doc |
|---|---|---|
| Sharded LRU (explicit invalidation semantics) | `edgion-gateway/src/cache/sharded.rs` | not yet written |
| TinyUFO + TTL (admission-friendly, no per-key delete) | `edgion-gateway/src/cache/tiny_ufo.rs` | not yet written |

The selection rule between these two is a 🔴 design rule, not a preference — see the
in-process cache selection section of [../00-design-rules.md](../00-design-rules.md).

### Matching

| Algorithm | Source | Doc |
|---|---|---|
| IP radix tree (allow/deny matching) | `edgion-resources/src/matcher/ip_radix_tree/` | not yet written |
| Aho-Corasick multi-pattern matching (WAF) | `edgion-waf/src/aho_corasick/` | not yet written |

### Rate limiting

| Algorithm | Source | Doc |
|---|---|---|
| Redis Lua rate limiter core | `edgion-gateway/src/plugins/common/redis_rate_limiter/` | not yet written |

## Entry template

Every entry follows this shape. Sections are mandatory; "None" is a valid body.

```markdown
---
name: <kebab-case-slug>
description: <one line: which algorithm, where it runs, and what decision the entry settles>
status: shipped | proposed | rejected
---

# <Algorithm> — <subsystem>

## What we run today
Exact math or pseudocode. Constants with units. Where it is called from.

## Contract
Complexity, allocation behavior, concurrency primitive (lock-free / lock / single-threaded),
saturation and overflow behavior, determinism and how it is tested.

## Why this algorithm
The requirement that selected it. What it deliberately does NOT optimize for.

## Prior art comparison
Table: each comparable implementation, what it does differently, and whether we adopt,
diverge, or reject — with the reason. Cite exact upstream source, not blog summaries.

## Known limitations
Failure modes we accept, with the operational symptom each produces.

## Replacement criteria
The measurement that would have to come back positive to justify switching. Without this
section an entry cannot settle a future "should we switch to X?" question.

## References
Source paths, upstream links, related tasks and review findings.
```

## Rules for writing an entry

1. **Cite upstream source, not summaries.** Quote the actual implementation file (Tower,
   Envoy, Linkerd, Finagle). Blog posts and docs pages routinely describe behavior the code
   does not have.
2. **State constants with units and derivation.** `half_life = 10s` is useless without why 10.
3. **Record rejections too.** An entry with `status: rejected` is as valuable as a shipped
   one — it stops the next agent from re-proposing it.
4. **Divergences from prior art must be justified inline**, in the comparison table, not in
   prose elsewhere. A divergence with no stated reason is a defect waiting to be "fixed" back.
5. **Benchmark numbers carry their harness.** A number without the machine, backend count,
   and load shape is not evidence.
6. **No hot-path observability creep.** Adding an entry here never licenses new per-endpoint
   metrics or request-path `tracing::` calls; see [../00-design-rules.md](../00-design-rules.md).
