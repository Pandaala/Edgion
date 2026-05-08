# Writing Style

## Person and tone

| Rule | Correct | Incorrect |
|------|---------|-----------|
| Address the reader as "you" | "You can configure …" | "Users can configure …" |
| Use active voice | "Gateway decrypts the TLS connection" | "The TLS connection is decrypted by Gateway" |
| Direct and concise | "Configure `allow_origins`" | "You need to go and configure the `allow_origins` field" |
| No meaningless padding | "Create a Secret" | "Please create a Secret resource" |

**Tone**: Professional but not stiff. Like technical conversation between colleagues, not a product manual.

## Heading rules

### H1

- It is the name of the feature, resource, or concept
- No redundant prefixes

| Correct | Incorrect |
|---------|-----------|
| `# CORS Plugin` | `# Edgion CORS Plugin Usage Guide` |
| `# TLS Termination` | `# Configuration Notes about TLS Termination` |
| `# HTTPRoute Overview` | `# HTTPRoute User Guide Overview Document` |

### Section headings

- Use verbs or noun phrases, not full sentences
- Name them by functional meaning, not by document position

| Correct | Incorrect |
|---------|-----------|
| `## Quick Start` | `## Section 1: Quick Start` |
| `## Configuration Parameters` | `## This Chapter Introduces Configuration Parameters` |
| `## FAQ` | `## The FAQ Section` |

## Opening patterns

### Edgion extension callout

For features that are Edgion extensions (not standard Gateway API), add a blockquote callout immediately after the H1:

```markdown
# CORS Plugin

> **🔌 Edgion extension**
> 
> CORS is a cross-origin configuration plugin provided by the `EdgionPlugins` CRD; it is not part of standard Gateway API.
```

The callout format is fixed:
- First line: `> **🔌 Edgion extension**`
- Empty line: `> `
- Description line: one sentence stating what CRD / what capability this is, and that it is not part of standard Gateway API

**Does not need a callout**: standard Gateway API features (HTTPRoute, Gateway, GatewayClass, etc.).

### Overview paragraph

After the callout (or, if there is none, after the H1), use 1–3 sentences to describe:
1. What this feature is
2. What problem it solves

Do not put configuration details in the overview paragraph.

## Technical terminology

### Terms not to translate

The following standard Gateway API terms remain in English in Chinese documentation:

- Gateway, GatewayClass, HTTPRoute, GRPCRoute, TCPRoute, UDPRoute, TLSRoute
- Listener, BackendRef, ParentRef, SectionName
- Secret, Namespace, ConfigMap, ReferenceGrant
- Filter, Match, Rule
- Endpoint, Service, Pod

### Annotate at first appearance

In Chinese documentation, Edgion-specific concepts are annotated with the original English in parentheses on first appearance — for example, the Chinese term for "control plane" should be followed by `(Controller)` the first time it appears, and the Chinese term for "data plane" should be followed by `(Gateway)`.

Subsequent uses can use Chinese alone.

### Names in code / configuration

YAML field names, CLI parameters, environment variables, etc. always use the original text wrapped in backticks. For example, in Chinese prose the field `allow_origins` is referenced as `` `allow_origins` ``, never translated or paraphrased.

## Content boundaries

### What docs/ may cover

- What a feature is and what problem it solves
- How to configure it (YAML examples)
- Configuration parameter descriptions
- Use cases and best practices
- FAQ and troubleshooting
- Relationship to other features

### What docs/ should not cover

- Internal implementation details (handler flows, data structures, source locations)
- Code-level design decisions
- Performance implementation mechanisms
- Concrete Rust code samples (dev-guide is the exception)

If internal knowledge truly needs to be referenced, link to the corresponding document under dev-guide/.

## Format conventions

### Emphasis

| Purpose | Format | Example |
|---------|--------|---------|
| Code, configuration fields, CLI commands | Backticks | `allow_origins`, `kubectl apply` |
| Important warning | Bold | **Do not use in production** |
| Feature / product name | Plain text | Gateway API, Edgion |
| Security warning | ⚠️ + bold | ⚠️ **For development only** |
| New concept definition | Bold | **TLS termination** refers to … |

### Lists

- Use ordered lists (1. 2. 3.) for steps
- Use unordered lists (-) to enumerate options
- End list items with a period (Chinese) or no punctuation (phrase form)

### Code blocks

- Tag YAML configurations with the `yaml` language
- Tag shell commands with `bash`
- Use `text` (or no language tag) for output / logs
- Use untagged code blocks for ASCII diagrams

### Right / wrong example markers

When contrasting correct and incorrect approaches, use ✅/❌:

```markdown
```yaml
✅ Correct:
allow_origins: "https://app.example.com"
allow_credentials: true

❌ Incorrect:
allow_origins: "*"
allow_credentials: true
``` (remove the backtick escape on the line above)
```

Or in a table:

```markdown
| Practice | Example |
|----------|---------|
| ✅ Recommended | `allow_origins: "https://app.example.com"` |
| ❌ Avoid | `allow_origins: "*"` |
```

---

## YAML example conventions

### Completeness requirement

**Every YAML example must be complete and runnable**. Readers should be able to copy-paste it directly and `kubectl apply` (or apply with minor edits).

```yaml
# ✅ Complete example: includes apiVersion, kind, metadata, spec
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: example-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api/
      backendRefs:
        - name: api-service
          port: 8080
```

**Exception**: The "Quick Start" and "Configuration scenarios" sections of plugin docs may show only a `filters` snippet, but the **complete configuration example** section must give a full YAML including the HTTPRoute envelope.

### Scenario ordering

From simple to complex:

1. Minimum viable configuration (development / quick verification)
2. Single scenario (most common production usage)
3. Multi-option combination
4. Configuration with security / authentication
5. Advanced / edge scenarios

Each scenario should have a heading describing its purpose.

### Comment rules

- Place comments above the field, not at the end of the line
- Chinese docs use Chinese comments; English docs use English comments
- Concise — one line should suffice
- Do not explain YAML syntax itself
- Skip comments for self-explanatory field names

```yaml
# ✅ Useful comment
filters:
  - type: Cors
    config:
      # Allowed frontend origins
      allow_origins: "https://app.example.com,https://admin.example.com"
      # Cache preflight requests for 24 hours
      max_age: 86400

# ❌ Redundant comment
filters:
  - type: Cors          # The type is Cors
    config:
      allow_origins: "*" # Set allow_origins to *
```

### Placeholder consistency

| Purpose | Placeholder | Avoid |
|---------|-------------|-------|
| Domain | `example.com`, `app.example.com`, `api.example.com` | `mysite.com`, `foo.bar` |
| Admin domain | `admin.example.com` | `management.xxx` |
| Namespace | `production`, `staging`, `edgion-test` | `my-ns`, `test123` |
| Service name | `api-service`, `echo-service`, `backend-service` | `svc1`, `my-svc` |
| Port | `8080` (HTTP), `443` (HTTPS), `80` (HTTP entry) | Arbitrary numbers |
| Secret name | `my-tls-secret`, `<purpose>-tls` | `secret1` |
| Gateway name | `main-gateway`, `internal-gateway` | `gw1` |
| Route name | `<purpose>-route`, e.g. `api-route`, `example-route` | `route1` |
| IP address | `192.168.1.x` (private), `10.0.0.x` (in-cluster) | `1.2.3.4` |
| Base64 data | `<base64-encoded-cert>` placeholder text | A real long base64 string |

### Security warnings

Examples involving security must be marked with environment applicability:

```markdown
⚠️ **For development only! Do not use in production!**
```

### YAML formatting

- Indent with 2 spaces
- Quote string values that contain special characters with double quotes
- Use `true`/`false` for booleans, not `yes`/`no`
- Use `[]` for empty lists and `{}` for empty objects
- Do not wrap long lines (avoid `>` or `|` multiline syntax unless truly necessary)

---

## Cross-reference conventions

### Relative path rules

All in-document references use **relative paths**, not absolute paths or URLs:

- ✅ Relative path (correct): display text plus a relative path, e.g. `[display text]` `(../overview.md)` — the path is relative to the current file
- ✅ Cross-directory relative path (correct): `[display text]` `(../../infrastructure/mtls.md)`
- ❌ Absolute path (incorrect): `/docs/zh-CN/user-guide/http-route/filters/overview.md`

- The link starting point is the directory of the current file
- Do not link across language directories (do not link from `en/` to `zh-CN/`)
- When linking to a directory entry, write README.md, e.g.: `[Backend configuration]` `(../backends/README.md)` (the path must point to an existing file)

### "Related documents" section

Place at the end of the page; list 3–6 most relevant links:

```text
## Related documents

- HTTPS listeners (relative path, e.g. ../listeners/https.md)
- EdgionTls extension (relative path, e.g. ./edgion-tls.md)
- mTLS configuration (relative path, e.g. ../../infrastructure/mtls.md)
```

Rules: order by relevance; the link text is the title of the target document; do not include a description.

### References in body text

Link the first time you mention another feature; do not repeat the link on subsequent mentions. At the end of a section, use "More on X: …" to direct readers to the detailed document.

### External links

Prefer linking to authoritative sources (Gateway API official, MDN, K8s official, RFC). Do not link to blogs, commercial pages, or articles likely to break.

### Between docs/ and skills/

`docs/` should **not** link to `skills/`. When you need architectural information, link to dev-guide/.

### Link maintenance

When moving / renaming / deleting files, search for and update all references. The review checklist includes a link-validity check.
