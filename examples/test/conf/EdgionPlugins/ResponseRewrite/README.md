# ResponseRewrite 集成测试

## 测试场景

### 1. 状态码修改 (`/status-code`)

测试将响应状态码从默认值修改为 201。

**配置**：`01_EdgionPlugins_status-code.yaml`

**验证**：
```bash
curl -i http://response-rewrite.example.com/status-code
# 期望: HTTP/1.1 201 Created
```

### 2. 响应头设置 (`/headers-set`)

测试响应头的 set、add、remove 操作。

**配置**：`02_EdgionPlugins_headers-set.yaml`

**验证**：
```bash
curl -i http://response-rewrite.example.com/headers-set
# 期望:
# - X-Custom-Header: custom-value
# - Cache-Control: no-cache, no-store
# - X-Powered-By: Edgion
# - Server 头被删除
```

### 3. 响应头重命名 (`/headers-rename`)

测试响应头重命名功能。

**配置**：`03_EdgionPlugins_headers-rename.yaml`

**验证**：
```bash
curl -i http://response-rewrite.example.com/headers-rename
# 期望:
# - X-Request-Id: <原 X-Internal-Id 的值>
# - X-Trace-Info: <原 X-Debug-Info 的值>
```

### 4. 综合功能 (`/combined`)

测试状态码 + 响应头综合操作。

**配置**：`04_EdgionPlugins_combined.yaml`

**验证**：
```bash
curl -i http://response-rewrite.example.com/combined
# 期望:
# - HTTP/1.1 200 OK
# - X-Request-Id: <原 X-Internal-Id 的值>
# - Cache-Control: no-cache
# - X-API-Version: v2
# - X-Powered-By: Edgion
# - Server 和 X-Debug 头被删除
```

## 运行测试

```bash
# 应用配置
kubectl apply -f .

# 运行集成测试
cd ../../script && ./run_integration_test.sh ResponseRewrite
```
