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

## Running the Tests

```bash
cd examples/test
./run_test.sh EdgionPlugins/RequestRestriction
```
