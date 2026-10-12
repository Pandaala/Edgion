---
name: graphql-static-safelist
description: Review bounded whole-document trust and verified selected-operation matching.
knowledge_type: regression-guards
---

# Static GraphQL trusted documents

GraphqlSafelist compiles at most128 bounded sources with canonical selection and
stores SHA-256 digests plus actual query/mutation/name membership. Source limits
are checked before copying/parsing. Runtime uses the same strict envelope,
BodyStorage, four-permit admission and carried Work as the other GraphQL plugins.
No expanded AST, document copy or selected-name allocation is added to lookup.

Fingerprints hash length-delimited retained Apollo lexer tokens. Every token,
including discarded whitespace/comments/commas/EOF, consumes carried Work.
Literals and block strings retain exact bytes; definition order is significant.
Variables' runtime values are excluded, but their definitions and document uses
are included. Whole-document digest and verified selected kind/name are both
required. A configured operationName cannot authorize modified fields or another
unregistered operation in the same document. Unknown trust returns fixed403.

This is a trusted-chain snapshot; rewrites precede inspection, later mutation is
outside its contract, and conditions/policies can bypass it. Existing whole-resource
DSL/WAF retention can preserve an earlier safelist when valid revocation accompanies
invalid DSL/WAF. Separate resources allow independent safelist updates; no new
retention mechanism or APQ cache is introduced.

See [user contract](../../../../docs/en/user-guide/http-route/filters/edgion-plugins/graphql-safelist.md),
[fingerprint](../../../../edgion-resources/src/graphql_request/safelist.rs),
[compiled membership](../../../../edgion-resources/src/resources/edgion_plugins/plugin_configs/graphql_safelist.rs)
and [runtime](../../../../edgion-gateway/src/plugins/http/graphql_safelist/mod.rs).
Re-evaluate lexer/parser upgrades, token representation, digest boundaries,
selection semantics, mutation ordering and retention changes against these gates.

## Canonical body admission freeze

Successful canonical GraphQL evaluation now calls the existing RequestBodyState
mutation freeze, preserving the inspected body and propagating refusal to a
subsequently installed bodyless replay owner. Guard measure mode also freezes
a successfully inspected body. Failed or skipped evaluation does not freeze.
Later body mutation through DSL/ProxyRewrite/Wasm or REST adaptation is refused;
multiple adapters before inspection remain allowed. Query/URI/method/header
changes still require the documented trusted-chain ordering. See the
[adapter/body ownership contract](../../http-body/regression-guards/rest-to-graphql-request-adaptation.md).
