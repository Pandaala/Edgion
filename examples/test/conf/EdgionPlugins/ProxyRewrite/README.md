# ProxyRewrite Integration Tests

## File Layout

```text
ProxyRewrite/
├── EdgionPlugins_default_proxy-rewrite.yaml  # Plugin configuration for all scenarios
├── HTTPRoute_default_proxy-rewrite.yaml      # Route configuration for all scenarios
└── README.md
```

## Test Scenarios

### 1. URI Rewrite (`/uri/*`)

| Path | Coverage |
|------|----------|
| `/uri/simple/*` | Replace the URI with a fixed value |
| `/uri/var/*` | Use the `$uri` variable |
| `/uri/arg/*` | Use `$arg_xxx` variables |

### 2. Regex URI Rewrite (`/regex/*`)

| Path | Coverage |
|------|----------|
| `/regex/users/:id` | Single capture group `$1` |
| `/regex/api/:type/:id/:action` | Multiple capture groups |
| `/regex/profile/:id` | Reuse a capture group in a header |

### 3. Host and Method Rewrite

| Path | Coverage |
|------|----------|
| `/host/rewrite/*` | Rewrite the `Host` header |
| `/method/to-post/*` | Convert `GET` to `POST` |
| `/combo/full/*` | Rewrite URI, host, and method together |

### 4. Header Operations (`/headers/*`)

| Path | Coverage |
|------|----------|
| `/headers/add/*` | Add headers |
| `/headers/set/*` | Set headers |
| `/headers/remove/*` | Remove headers |
| `/headers/combo/*` | Combine add, set, and remove |

### 5. Path Parameter Variables (`/params/*`)

| Route Pattern | Coverage |
|---------------|----------|
| `/params/uri/:uid` | Use `$uid` in the URI |
| `/params/header/:uid/:action` | Use multiple path params in headers |
| `/params/mixed/:service/:resource` | Combine path params with query params |

### 6. Combined Tests (`/full/*`)

| Route Pattern | Coverage |
|---------------|----------|
| `/full/api/:uid` | Full API gateway rewrite flow |
| `/full/query/*` | Preserve the query string |

## Supported Variables

| Variable | Meaning | Example |
|----------|---------|---------|
| `$uri` | Original request path | `/api/v1/users` |
| `$arg_<name>` | Query parameter | `$arg_keyword` |
| `$1-$9` | Regex capture groups | `$1`, `$2` |
| `$<name>` | Path parameter | `$uid`, `$service` |

## Test Commands

```bash
# Host: proxy-rewrite.example.com

# URI rewrite
curl -H "Host: proxy-rewrite.example.com" http://localhost:31180/uri/simple/test
curl -H "Host: proxy-rewrite.example.com" http://localhost:31180/uri/arg/test?keyword=hello&lang=en

# Regex rewrite
curl -H "Host: proxy-rewrite.example.com" http://localhost:31180/regex/users/123

# Path parameters
curl -H "Host: proxy-rewrite.example.com" http://localhost:31180/params/uri/456/data
curl -H "Host: proxy-rewrite.example.com" http://localhost:31180/params/header/789/edit

# Combined flow
curl -H "Host: proxy-rewrite.example.com" http://localhost:31180/full/api/999/profile?trace_id=abc123
```
