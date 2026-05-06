# TODO: CEL Validation Rules for Plugin CRDs

## Background

Kubernetes 1.25+ supports `x-kubernetes-validations` (CEL rules) directly in CRD schemas.
CEL handles cross-field constraints without admission webhooks — now the industry standard
for complex CRD validation.

CRD schema (`schemars`) handles scalar constraints (min/max/pattern).
CEL handles cross-field / conditional constraints that schemars cannot express.

## Plugins Needing CEL Rules

### CorsConfig

```yaml
x-kubernetes-validations:
  # allow_credentials=true conflicts with wildcard "*" in any of the 5 allow_* fields
  - rule: >
      !(self.allowCredentials == true &&
        (self.allowOrigins.orValue([]).exists(o, o == "*") ||
         self.allowMethods.orValue([]).exists(m, m == "*") ||
         self.allowHeaders.orValue([]).exists(h, h == "*") ||
         self.exposeHeaders.orValue([]).exists(h, h == "*")))
    message: "allowCredentials cannot be true when any of allowOrigins/allowMethods/allowHeaders/exposeHeaders contains '*'"
```

**Fields involved**: `allow_credentials`, `allow_origins`, `allow_methods`, `allow_headers`, `expose_headers`

---

### JwtAuthConfig

```yaml
x-kubernetes-validations:
  # When anonymous is not set, at least one credential source must be present
  - rule: >
      has(self.anonymous) ||
      has(self.secretRef) ||
      has(self.secretRefs)
    message: "At least one of anonymous, secretRef, or secretRefs must be set"
```

**Fields involved**: `anonymous`, `secret_ref`, `secret_refs`

---

### RateLimitConfig

```yaml
x-kubernetes-validations:
  # skew_tolerance only meaningful when scope=Cluster; warn users who set it with Instance
  - rule: >
      self.scope == "Cluster" || self.skewTolerance == 1.2
    message: "skewTolerance has no effect when scope is Instance"
  # skew_tolerance range: 1.0 to 2.0
  - rule: self.skewTolerance >= 1.0 && self.skewTolerance <= 2.0
    message: "skewTolerance must be between 1.0 and 2.0"
```

**Fields involved**: `scope`, `skew_tolerance`

---

### RateLimitRedisConfig

```yaml
x-kubernetes-validations:
  # redis_ref must be "namespace/name" format
  - rule: self.redisRef.matches('^[a-z0-9-]+/[a-z0-9-]+$')
    message: "redisRef must be in 'namespace/name' format"
  # policies must be non-empty
  - rule: size(self.policies) > 0
    message: "at least one rate limit policy is required"
```

**Fields involved**: `redis_ref`, `policies`

---

### HmacAuthConfig

```yaml
x-kubernetes-validations:
  # algorithms list must be non-empty
  - rule: size(self.algorithms) > 0
    message: "algorithms list must not be empty"
```

**Fields involved**: `algorithms`

---

## Implementation Steps

1. **Generate CRD JSON Schema**: run `cargo test` or the dedicated CRD-gen binary to produce
   the OpenAPI v3 schema for each plugin config type.

2. **Add CEL rules**: for each plugin CRD type, add `x-kubernetes-validations` entries at the
   appropriate struct level (plugin config object, not field level).

3. **Test with dry-run**: `kubectl apply --dry-run=server -f <plugin-cr.yaml>` to verify CEL
   rules fire correctly before merging.

4. **Minimum Kubernetes version**: CEL in CRDs requires K8s 1.25+ (GA). Document this requirement.

## References

- [KEP-2876: CRD Validation Expression Language](https://github.com/kubernetes/enhancements/tree/master/keps/sig-api-machinery/2876-crd-validation-expression-language)
- [Kubernetes docs: CRD validation rules](https://kubernetes.io/docs/tasks/extend-kubernetes/custom-resources/custom-resource-definitions/#validation-rules)
- [CEL language spec](https://github.com/google/cel-spec)
