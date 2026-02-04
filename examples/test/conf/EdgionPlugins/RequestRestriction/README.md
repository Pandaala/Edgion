# RequestRestriction 集成测试

## 测试用例

### 1. Header 拒绝列表 (header-deny)

阻止包含 Bot/Spider/Crawler 的 User-Agent：

```bash
# 正常请求 - 应该通过 (200)
curl -H "Host: request-restriction.example.com" \
     -H "User-Agent: Mozilla/5.0" \
     http://localhost:8080/test/header-deny/api

# Bot 请求 - 应该被拒绝 (403)
curl -H "Host: request-restriction.example.com" \
     -H "User-Agent: Googlebot/2.1" \
     http://localhost:8080/test/header-deny/api
```

### 2. Path 允许列表 (path-allow)

只允许 /api/* 和 /health 路径：

```bash
# 允许的路径 - 应该通过 (200)
curl -H "Host: request-restriction.example.com" \
     http://localhost:8080/test/path-allow/api/users

curl -H "Host: request-restriction.example.com" \
     http://localhost:8080/test/path-allow/health

# 不允许的路径 - 应该被拒绝 (404)
curl -H "Host: request-restriction.example.com" \
     http://localhost:8080/test/path-allow/admin/users
```

### 3. Method 允许列表 (method-allow)

只允许 GET/HEAD/OPTIONS 方法：

```bash
# GET 请求 - 应该通过 (200)
curl -X GET -H "Host: request-restriction.example.com" \
     http://localhost:8080/test/method-allow/api

# POST 请求 - 应该被拒绝 (405)
curl -X POST -H "Host: request-restriction.example.com" \
     http://localhost:8080/test/method-allow/api
```

### 4. Header 必须存在 (header-required)

要求 X-Auth-Token 头存在且不是 invalid/expired：

```bash
# 有效 Token - 应该通过 (200)
curl -H "Host: request-restriction.example.com" \
     -H "X-Auth-Token: valid-token-123" \
     http://localhost:8080/test/header-required/api

# 无 Token - 应该被拒绝 (401)
curl -H "Host: request-restriction.example.com" \
     http://localhost:8080/test/header-required/api

# 无效 Token - 应该被拒绝 (401)
curl -H "Host: request-restriction.example.com" \
     -H "X-Auth-Token: invalid" \
     http://localhost:8080/test/header-required/api
```

### 5. 综合测试 (combined)

多规则组合，任一规则触发即拒绝：

```bash
# 正常请求 - 应该通过 (200)
curl -H "Host: request-restriction.example.com" \
     -H "User-Agent: Mozilla/5.0" \
     http://localhost:8080/test/combined/api/users

# Bot UA - 应该被拒绝 (403)
curl -H "Host: request-restriction.example.com" \
     -H "User-Agent: Googlebot" \
     http://localhost:8080/test/combined/api/users

# Admin 路径 - 应该被拒绝 (403)
curl -H "Host: request-restriction.example.com" \
     http://localhost:8080/test/combined/admin/users

# Debug Cookie - 应该被拒绝 (403)
curl -H "Host: request-restriction.example.com" \
     -H "Cookie: debug=true" \
     http://localhost:8080/test/combined/api/users
```

## 运行测试

```bash
cd examples/test
./run_test.sh EdgionPlugins/RequestRestriction
```
