# RequestRestriction Integration Tests

## Test Cases

### 1. Header Deny List (`header-deny`)

Blocks requests whose `User-Agent` contains `Bot`, `Spider`, or `Crawler`.

```bash
# Normal request, should pass (200)
curl -H "Host: request-restriction.example.com" \
     -H "User-Agent: Mozilla/5.0" \
     http://localhost:8080/test/header-deny/api

# Bot request, should be rejected (403)
curl -H "Host: request-restriction.example.com" \
     -H "User-Agent: Googlebot/2.1" \
     http://localhost:8080/test/header-deny/api
```

### 2. Path Allow List (`path-allow`)

Allows only `/api/*` and `/health`.

```bash
# Allowed paths, should pass (200)
curl -H "Host: request-restriction.example.com" \
     http://localhost:8080/test/path-allow/api/users

curl -H "Host: request-restriction.example.com" \
     http://localhost:8080/test/path-allow/health

# Disallowed path, should be rejected (404)
curl -H "Host: request-restriction.example.com" \
     http://localhost:8080/test/path-allow/admin/users
```

### 3. Method Allow List (`method-allow`)

Allows only `GET`, `HEAD`, and `OPTIONS`.

```bash
# GET request, should pass (200)
curl -X GET -H "Host: request-restriction.example.com" \
     http://localhost:8080/test/method-allow/api

# POST request, should be rejected (405)
curl -X POST -H "Host: request-restriction.example.com" \
     http://localhost:8080/test/method-allow/api
```

### 4. Required Header (`header-required`)

Requires `X-Auth-Token` to exist and rejects `invalid` or `expired`.

```bash
# Valid token, should pass (200)
curl -H "Host: request-restriction.example.com" \
     -H "X-Auth-Token: valid-token-123" \
     http://localhost:8080/test/header-required/api

# Missing token, should be rejected (401)
curl -H "Host: request-restriction.example.com" \
     http://localhost:8080/test/header-required/api

# Invalid token, should be rejected (401)
curl -H "Host: request-restriction.example.com" \
     -H "X-Auth-Token: invalid" \
     http://localhost:8080/test/header-required/api
```

### 5. Combined Rules (`combined`)

Combines multiple rules and rejects the request when any rule matches.

```bash
# Normal request, should pass (200)
curl -H "Host: request-restriction.example.com" \
     -H "User-Agent: Mozilla/5.0" \
     http://localhost:8080/test/combined/api/users

# Bot UA, should be rejected (403)
curl -H "Host: request-restriction.example.com" \
     -H "User-Agent: Googlebot" \
     http://localhost:8080/test/combined/api/users

# Admin path, should be rejected (403)
curl -H "Host: request-restriction.example.com" \
     http://localhost:8080/test/combined/admin/users

# Debug cookie, should be rejected (403)
curl -H "Host: request-restriction.example.com" \
     -H "Cookie: debug=true" \
     http://localhost:8080/test/combined/api/users
```

### 10. ipRefs in a rule group (`iprefs-group`)

Denies `/admin/` only when the peer is in the referenced IpList `rr-ipref-list` (`127.0.0.0/8`).

### 11. Entry-level `run` ipRefs (`iprefs-run`)

`RequestHeaderModifier` runs only for peers in `rr-ipref-list` and sets `X-IpRef-Run`. `RequestRestriction` then denies `/gated`.

### 12. HMAC Allow (`rr-hmac`)

Allows only requests that pass HMAC verification against `rr-hmac-credentials`. Unsigned or bad signatures are 403.

### 13. Missing Deny `ipRefs` (`missing-ipref-deny`)

An unresolved `ipRefs` dependency in `RequestRestriction` `Deny` mode returns the generic 500 response instead of under-blocking.

### 14. Missing Entry-level `run` `ipRefs` (`missing-ipref-run`)

An unresolved `ipRefs` dependency in entry-level `conditions.run` seals the EdgionPlugins runtime, so the route returns the generic 500 response.

## Running the Tests

```bash
cd edgion-tests
./run_test.sh EdgionPlugins/RequestRestriction
```
