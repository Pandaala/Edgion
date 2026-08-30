# HTTPRoute Match Rules

Match rules decide which requests a routing rule applies to. A rule's `matches` can test the request path, headers, query parameters, and HTTP method.

## How matches combine

- **Conditions within one match are ANDed**: a single match block that sets `path`, `headers`, and `method` requires all of them to hold.
- **Multiple matches in one rule are ORed**: a request matches the rule if it satisfies any one of the match blocks.
- **Most-specific match wins**: when more than one rule matches a request, Edgion picks the most specific one — exact paths and longer prefixes take precedence over shorter prefixes.
- **An omitted `path` means `PathPrefix /`**: a match block that only sets `headers`, `queryParams`, or `method` applies to every path. This holds for all config-center backends (Kubernetes, File, Etcd).

```yaml
rules:
  - matches:
      # Match A: GET requests under /api with a JWT auth header (all three ANDed)
      - path:
          type: PathPrefix
          value: /api
        method: GET
        headers:
          - name: X-Auth-Type
            value: jwt
      # Match B: any request under /public (ORed with Match A)
      - path:
          type: PathPrefix
          value: /public
    backendRefs:
      - name: api-service
        port: 8080
```

## Case sensitivity

| Dimension | Case behavior |
|-----------|---------------|
| Path | Case-sensitive |
| Header name | Case-insensitive (HTTP header names are case-insensitive) |
| Header value | Case-sensitive |
| Query parameter name and value | Case-sensitive |
| Method | Uppercase tokens (`GET`, `POST`, …) |

## Subdocuments

- [Path matching](./path.md)
- [Header matching](./headers.md)
- [Query parameter matching](./query-params.md)
- [HTTP method matching](./method.md)
