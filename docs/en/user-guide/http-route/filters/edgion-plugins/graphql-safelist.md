# GraphqlSafelist

GraphqlSafelist authorizes a static set of complete GraphQL documents and their
actual selected operations. Place it in requestPlugins after authentication and
all request rewrites. Later plugins must not change the admitted document.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: trusted-graphql
spec:
  requestPlugins:
    - type: GraphqlSafelist
      config:
        operations:
          - query: |
              query Users($first: Int!) {
                users(first: $first) { id name }
              }
          - query: |
              query Read { ping }
              mutation Write { update }
            operationName: Read
```

Clients still send the complete query through strict GraphQL GET or JSON POST.
Whitespace, comments and insignificant commas may differ. Every retained token
is length-delimited before SHA-256 hashing. Literal/string/block-string bytes,
variable definitions/defaults, aliases, directives, fragments and definition
order must match. Equivalent escaped strings or reordered definitions are
intentionally treated as different documents. Runtime variable values may differ.

Authorization also checks the actual selected query/mutation kind and name.
Omit operationName in a registration only when that document has one operation.
A named single operation can be selected implicitly or explicitly by clients.
To allow both operations in a multi-operation document, register the same full
document twice with different operationName values. Registering Read in the
example never permits selecting Write or replacing Read's fields. Anonymous
operations require an anonymous registered document.

Configure 1..128 entries, at most 64KiB per source and 512KiB of source overall;
selectors are at most 256 bytes. Duplicate canonical document/selected-operation
pairs and invalid documents reject configuration. The existing 64KiB wire
envelope, parser/token/recursion/work limits and four shared evaluation permits
also apply. Fingerprinting carries the parsing Work ledger. Unknown documents or
unregistered selections return 403; invalid requests retain their existing
400/405/413/415 statuses. See [JSON errors](graphql-errors.md).

Accepted requests retain their original bytes and URLs. No SDL or Redis is
required. This feature does not perform full backend GraphQL validation, dynamic
registration, hash-only APQ requests, response caching or GraphQL execution.
Use GraphqlGuard and GraphqlRateLimit separately for shape and quota protection.

Conditions and policy bypass can skip this protection. Valid updates replace the
immutable safelist; invalid safelist configuration fails closed. The existing
whole-resource DSL/WAF retention policy can retain an earlier safelist if a valid
revocation is bundled with invalid DSL/WAF changes. Use a separate EdgionPlugins
resource for the safelist when its revocation must be independent of those updates.
