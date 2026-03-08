# GRPCRoute Filters

GRPCRoute can use standard Gateway API filters and Edgion extension filtering capabilities (based on current implementation support).

## Recommended Strategy

1. Use standard filters first to meet common requirements.
2. Introduce Edgion plugin extensions as needed.
3. Be explicit about filter ordering to avoid conflicts between header rewriting and authentication.

## Related Documentation

- [HTTPRoute Filters Overview](../../http-route/filters/overview.md)
- [Plugin Composition and References](../../http-route/filters/plugin-composition.md)
