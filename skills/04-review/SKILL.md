---
name: review
description: Find Edgion review knowledge by owning function and decision type before validating or fixing a finding.
---

# Review knowledge

Start with the owning function below, then read the relevant design constraints,
accepted choices, compatibility limitations, regression guards, or counter-evidence.
The [routing rules](knowledge-routing.md) define classification, authority, and how to
rewrite an existing rule. The [template](_template.md) is for new or substantially
rewritten pages. Finding IDs are searchable provenance inside those pages.

Use [fix-review-issue.skill](fix-review-issue.skill) for one finding and
[global review](../10-fully-review/SKILL.md) for broad read-only reviews.
The mandatory graded design index remains [00-design-rules.md](../00-design-rules.md).

## Find the owning function

| Function | Entry |
|---|---|
| acme | [acme](acme/SKILL.md) |
| admin-api | [admin-api](admin-api/SKILL.md) |
| ai-quota | [ai-quota](ai-quota/SKILL.md) |
| ai-routing | [ai-routing](ai-routing/SKILL.md) |
| auth-timing | [auth-timing](auth-timing/SKILL.md) |
| backends | [backends](backends/SKILL.md) |
| basic-auth | [basic-auth](basic-auth/SKILL.md) |
| concurrency | [concurrency](concurrency/SKILL.md) |
| conf-sync | [conf-sync](conf-sync/SKILL.md) |
| controller | [controller](controller/SKILL.md) |
| cors | [cors](cors/SKILL.md) |
| cpu-memory | [cpu-memory](cpu-memory/SKILL.md) |
| csrf | [csrf](csrf/SKILL.md) |
| ctx-set | [ctx-set](ctx-set/SKILL.md) |
| deserialization | [deserialization](deserialization/SKILL.md) |
| dsl | [dsl](dsl/SKILL.md) |
| ext-proc | [ext-proc](ext-proc/SKILL.md) |
| federation | [federation](federation/SKILL.md) |
| forward-auth | [forward-auth](forward-auth/SKILL.md) |
| h2-grpc | [h2-grpc](h2-grpc/SKILL.md) |
| handler-requeue | [handler-requeue](handler-requeue/SKILL.md) |
| header-cert-auth | [header-cert-auth](header-cert-auth/SKILL.md) |
| hmac-auth | [hmac-auth](hmac-auth/SKILL.md) |
| host-header | [host-header](host-header/SKILL.md) |
| http-body | [http-body](http-body/SKILL.md) |
| http1 | [http1](http1/SKILL.md) |
| ip-restriction | [ip-restriction](ip-restriction/SKILL.md) |
| jwe-decrypt | [jwe-decrypt](jwe-decrypt/SKILL.md) |
| jwt-auth | [jwt-auth](jwt-auth/SKILL.md) |
| key-auth | [key-auth](key-auth/SKILL.md) |
| lb | [lb](lb/SKILL.md) |
| ldap-auth | [ldap-auth](ldap-auth/SKILL.md) |
| link-sys | [link-sys](link-sys/SKILL.md) |
| listeners | [listeners](listeners/SKILL.md) |
| observability | [observability](observability/SKILL.md) |
| oidc | [oidc](oidc/SKILL.md) |
| packaging | [packaging](packaging/SKILL.md) |
| plugin-metadata | [plugin-metadata](plugin-metadata/SKILL.md) |
| plugin-runtime | [plugin-runtime](plugin-runtime/SKILL.md) |
| rate-limit | [rate-limit](rate-limit/SKILL.md) |
| real-ip | [real-ip](real-ip/SKILL.md) |
| redirect | [redirect](redirect/SKILL.md) |
| request-mirror | [request-mirror](request-mirror/SKILL.md) |
| request-restriction | [request-restriction](request-restriction/SKILL.md) |
| resources | [resources](resources/SKILL.md) |
| review-method | [review-method](review-method/SKILL.md) |
| routing | [routing](routing/SKILL.md) |
| security | [security](security/SKILL.md) |
| sse | [sse](sse/SKILL.md) |
| ssrf | [ssrf](ssrf/SKILL.md) |
| status | [status](status/SKILL.md) |
| supply-chain | [supply-chain](supply-chain/SKILL.md) |
| testing | [testing](testing/SKILL.md) |
| tls | [tls](tls/SKILL.md) |
| uri-normalization | [uri-normalization](uri-normalization/SKILL.md) |
| waf | [waf](waf/SKILL.md) |
| wasm | [wasm](wasm/SKILL.md) |
| webhook-key | [webhook-key](webhook-key/SKILL.md) |
| websocket | [websocket](websocket/SKILL.md) |

## Find a decision

Use `rg -n '<symbol|behavior|finding-id>' skills/04-review/` to locate a specific
precedent. Read its conclusion, scope, evidence, and re-evaluation conditions. A match
in `superseded` is historical context, never grounds to suppress a current defect.
No root or module ledger is required; update the function's semantic knowledge page.
