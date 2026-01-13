# Gateway 动态性测试配置

本目录包含用于测试 Gateway 资源动态更新能力的配置文件。

## 目录结构

```
DynamicTest/
├── initial/          # 初始配置（第一次加载）
├── updates/          # 动态更新配置（第二次加载）
├── delete/           # 需要删除的资源列表
└── README.md         # 本文件
```

## 测试场景

### 场景 1: Gateway Hostname 约束动态移除

**端口**: 31250 (http-with-hostname listener)

| 阶段 | 配置 | 测试请求 | 预期结果 |
|------|------|----------|----------|
| 初始 | listener hostname=`api.example.com` | Host: `other.example.com` → `/match` | **404** (hostname 不匹配) |
| 更新后 | 移除 hostname 限制 | Host: `other.example.com` → `/match` | **200** (限制已移除) |

**配置文件**:
- Initial: `01_Gateway.yaml` (listener 带 hostname)
- Update: `Gateway_remove_hostname.yaml` (移除 hostname)

**验证点**: 之前被 hostname 限制拒绝的请求，动态更新后应成功。

---

### 场景 2: AllowedRoutes 从 Same 变为 All

**端口**: 31251 (http-same-ns listener)

| 阶段 | 配置 | 测试路由 | 预期结果 |
|------|------|----------|----------|
| 初始 | AllowedRoutes=`Same` | HTTPRoute (edgion-other namespace) | **404** (不同 namespace) |
| 更新后 | AllowedRoutes=`All` | 同一 HTTPRoute | **200** (跨 namespace 允许) |

**配置文件**:
- Initial: `01_Gateway.yaml` (AllowedRoutes=Same), `HTTPRoute_cross_namespace.yaml`
- Update: `Gateway_remove_hostname.yaml` (AllowedRoutes=All)

**验证点**: 跨 namespace 路由从不可达变为可达。

---

### 场景 3: HTTPRoute 动态添加

**端口**: 31252 (http-general listener)

| 阶段 | 配置 | 测试请求 | 预期结果 |
|------|------|----------|----------|
| 初始 | 无 `/new-api` 路由 | GET `/new-api` | **404** (路由不存在) |
| 更新后 | 添加 HTTPRoute for `/new-api` | GET `/new-api` | **200** (新路由生效) |

**配置文件**:
- Initial: 无
- Update: `HTTPRoute_add_new.yaml`

**验证点**: 新路由立即生效。

---

### 场景 4: HTTPRoute Match 规则动态修改

**端口**: 31252 (http-general listener)

| 阶段 | 配置 | 测试请求 | 预期结果 |
|------|------|----------|----------|
| 初始 | HTTPRoute match `GET /api/v1/*` | POST `/api/v1/users` | **404** (方法不匹配) |
| 更新后 | HTTPRoute match `POST /api/v1/*` | POST `/api/v1/users` | **200** (规则已更新) |

**配置文件**:
- Initial: `HTTPRoute_get_only.yaml` (method=GET)
- Update: `HTTPRoute_update_match.yaml` (method=POST)

**验证点**: 路由匹配规则更新后立即生效。

---

### 场景 5: HTTPRoute 动态删除

**端口**: 31252 (http-general listener)

| 阶段 | 配置 | 测试请求 | 预期结果 |
|------|------|----------|----------|
| 初始 | HTTPRoute for `/temp` | GET `/temp` | **200** (路由存在) |
| 更新后 | 删除该 HTTPRoute | GET `/temp` | **404** (路由已删除) |

**配置文件**:
- Initial: `HTTPRoute_temp.yaml`
- Delete: `delete/resources_to_delete.txt` (包含 HTTPRoute/edgion-test/route-temp)

**验证点**: 删除路由后立即不可达。

---

## 使用方法

### 1. 初始加载

```bash
# 仅加载 initial/ 目录（load_conf.sh 会自动排除 updates/ 和 delete/）
./examples/test/scripts/utils/load_conf.sh Gateway/DynamicTest
```

### 2. 运行初始阶段测试

```bash
./target/debug/examples/test_client -g -r Gateway -i Dynamic --phase initial
```

### 3. 动态更新配置

```bash
# 应用更新
./target/debug/edgion-ctl --server http://127.0.0.1:5800 \
    apply -f examples/test/conf/Gateway/DynamicTest/updates/

# 删除资源
while read -r resource; do
    [ -z "$resource" ] || [[ "$resource" =~ ^# ]] && continue
    ./target/debug/edgion-ctl --server http://127.0.0.1:5800 delete "$resource"
done < examples/test/conf/Gateway/DynamicTest/delete/resources_to_delete.txt

# 验证资源同步
./target/debug/examples/resource_diff \
    --controller-url http://127.0.0.1:5800 \
    --gateway-url http://127.0.0.1:5900

# 等待配置生效
sleep 3
```

### 4. 运行更新后测试

```bash
./target/debug/examples/test_client -g -r Gateway -i Dynamic --phase update
```

### 5. 完整流程（自动化）

```bash
./examples/test/scripts/integration/run_integration.sh -r Gateway --dynamic-test
```

## 端口分配

- **31250**: http-with-hostname (场景 1 - hostname 约束)
- **31251**: http-same-ns (场景 2 - AllowedRoutes)
- **31252**: http-general (场景 3、4、5 - HTTPRoute CRUD)

## 注意事项

1. **顺序依赖**: 必须先加载 initial/ 配置并运行初始测试，再加载 updates/ 配置
2. **资源清理**: 测试完成后建议重启服务或手动清理资源
3. **时序要求**: 动态更新后需等待 2-3 秒确保配置生效（ArcSwap 机制）
4. **幂等性**: 多次运行可能需要先清理之前的测试资源
