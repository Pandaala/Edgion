# ResponseRewrite Integration Tests

## Test Scenarios

### 1. Status Code Rewrite (`/status-code`)

Changes the upstream response status code to `201`.

**Configuration:** `01_EdgionPlugins_status-code.yaml`

**Verification:**

```bash
curl -i http://response-rewrite.example.com/status-code
# Expect: HTTP/1.1 201 Created
```

### 2. Response Header Updates (`/headers-set`)

Verifies `set`, `add`, and `remove` operations on response headers.

**Configuration:** `02_EdgionPlugins_headers-set.yaml`

**Verification:**

```bash
curl -i http://response-rewrite.example.com/headers-set
# Expect:
# - X-Custom-Header: custom-value
# - Cache-Control: no-cache, no-store
# - X-Powered-By: Edgion
# - The Server header is removed
```

### 3. Response Header Rename (`/headers-rename`)

Verifies response header rename behavior.

**Configuration:** `03_EdgionPlugins_headers-rename.yaml`

**Verification:**

```bash
curl -i http://response-rewrite.example.com/headers-rename
# Expect:
# - X-Request-Id: <value copied from X-Internal-Id>
# - X-Trace-Info: <value copied from X-Debug-Info>
```

### 4. Combined Flow (`/combined`)

Verifies status code and response header operations together.

**Configuration:** `04_EdgionPlugins_combined.yaml`

**Verification:**

```bash
curl -i http://response-rewrite.example.com/combined
# Expect:
# - HTTP/1.1 200 OK
# - X-Request-Id: <value copied from X-Internal-Id>
# - Cache-Control: no-cache
# - X-API-Version: v2
# - X-Powered-By: Edgion
# - Server and X-Debug headers are removed
```

## Running the Tests

```bash
# Apply the manifests
kubectl apply -f .

# Run the integration suite
cd ../../script && ./run_integration_test.sh ResponseRewrite
```
