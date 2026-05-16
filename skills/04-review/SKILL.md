---
name: review
description: Review-specific knowledge for Edgion. Use when auditing code review findings, especially to check whether a reported issue is a real bug, a project-accepted tradeoff, or a known false positive. Includes observability review rules (data-plane zero tracing, metrics explosion prevention, access log quality).
---

# 04 Review

> Per-finding review rules and decision notes. Each topic sub-directory holds concrete decision criteria, false-positive verdicts, and accepted tradeoffs. For closed findings (previously fixed or accepted) see [CLOSED-FINDINGS.md](CLOSED-FINDINGS.md). For global-codebase review workflow (6-subagent batch pattern) see [10-fully-review/](../10-fully-review/SKILL.md).

## Review issue fix workflow

To process a single finding from any `tasks/review-*/` directory, use [`fix-review-issue.skill`](fix-review-issue.skill).
Invoke it (or the `/fix-issue` global slash command) when the user says "review issue", "handle issue", "next issue", or any equivalent phrasing in another language.
Supports auto-detection of the issue directory, issue ID lookup, or explicit path arguments.

## Routing by topic

| Topic | Sub-directory / file |
|-------|----------------------|
| **Security iron rules** (timing, weak passwords, cache keys — cross-feature) | [security/iron-rules.md](security/iron-rules.md) |
| **Authentication** (basic / key / jwt / hmac / oidc / cert / forward / webhook) | [auth-timing/](auth-timing/), [hmac-auth/](hmac-auth/), [oidc/](oidc/), [local-auth/](local-auth/), [header-cert-auth/](header-cert-auth/), [forward-auth/](forward-auth/), [webhook-key/](webhook-key/), [admin-cors/](admin-cors/), [cors/](cors/) |
| **Network access control** | [ip-restriction/](ip-restriction/), [real-ip/](real-ip/), [ssrf/](ssrf/) |
| **TLS** (downstream / upstream / backends / EdgionTls) | [edgion-tls/](edgion-tls/), [ssl/](ssl/) |
| **URI / routing** | [uri-normalization/SKILL.md](uri-normalization/SKILL.md) |
| **Rate limiting** | [rate-limit/](rate-limit/) |
| **Concurrency** | [concurrency/](concurrency/), [rwlock-unwrap/](rwlock-unwrap/) |
| **CPU / memory performance** | [cpu-memory/](cpu-memory/), [memory-leak/](memory-leak/) |
| **Observability / logging** | [observability/](observability/) |
| **DSL** (Value type, regex) | [dsl-value-str/](dsl-value-str/), [dsl-regex/](dsl-regex/) |
| **Architecture decisions** | [architecture/](architecture/) |
| **Resource handler / requeue** | [handler-requeue/](handler-requeue/) |
| **Plugin runtime** | [ctx-set/](ctx-set/) |
| **Deserialization safety** | [deserialization/](deserialization/) |
| **Supply chain / dependencies** | [supply-chain/](supply-chain/) |

## Decision-rule template

New review-rule entries should follow the canonical shape in [`_template.md`](_template.md) (Conclusion → Core Rationale → Fix Suggestions Not Accepted → Re-evaluation Triggers → Reference Cases).

## Closed findings log

See [`CLOSED-FINDINGS.md`](CLOSED-FINDINGS.md) for per-issue closure history (14 entries from 04-review inline + 3 CSRF entries from 10-fully-review).

## Usage principles

- Only write here after the conclusion has been verified against the current code and design semantics.
- Keep content brief; only retain reusable judgment criteria, do not copy entire reviews.
- If design goals change in the future, update the conclusions here in sync.
