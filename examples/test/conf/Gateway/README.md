# Gateway 集成测试说明

本目录包含 Gateway API 相关的集成测试配置文件和测试用例。

## 测试套件结构

### 1. Listener Hostname 约束测试 (`ListenerHostname/`)

测试 Gateway Listener 的 hostname 约束功能：

- **精确匹配**: `api.example.com` 精确匹配 Listener hostname
- **通配符匹配**: `*.wildcard.example.com` 匹配通配符 Listener hostname
- **通配符不匹配根域名**: `*.wildcard.example.com` 不匹配 `wildcard.example.com`
- **无限制**: Listener 未指定 hostname，允许任意 HTTPRoute hostname

**端口**: 31240-31242
**测试入口**: `./test_client -g -r Gateway -i ListenerHostname`

### 2. AllowedRoutes 测试 (`AllowedRoutes/`)

测试 Gateway Listener 的 AllowedRoutes 约束功能：

#### 2.1 Same Namespace (`AllowedRoutes/Same/`)

测试 `allowedRoutes.namespaces.from: Same` 约束：

- ✅ 同 namespace 的 Route 允许访问
- ❌ 不同 namespace 的 Route 被拒绝（404）

**端口**: 31210
**测试入口**: `./test_client -g -r Gateway -i AllowedRoutes/Same`

#### 2.2 All Namespaces (`AllowedRoutes/All/`)

测试 `allowedRoutes.namespaces.from: All` 约束：

- ✅ 同 namespace 的 Route 允许访问
- ✅ 不同 namespace 的 Route 也允许访问

**端口**: 31211
**测试入口**: `./test_client -g -r Gateway -i AllowedRoutes/All`

#### 2.3 Kinds (`AllowedRoutes/Kinds/`)

测试 `allowedRoutes.kinds` 约束（只允许 HTTPRoute）：

- ✅ HTTPRoute 允许访问
- ❌ GRPCRoute 被拒绝

**端口**: 31213
**测试入口**: `./test_client -g -r Gateway -i AllowedRoutes/Kinds`

#### 2.4 Selector (`AllowedRoutes/Selector/`)

测试 `allowedRoutes.namespaces.from: Selector` 约束：

- ✅ 带有匹配 label 的 namespace 中的 Route 允许访问
- ❌ 不匹配 selector 的跨 namespace Route 被拒绝

本仓库当前 fixture 中：
- `edgion-test` namespace 带有 `env=prod`
- Gateway 位于 `edgion-test`
- `selector-same-ns-route` 允许访问
- `selector-cross-ns-route` 位于 `edgion-default`，应返回 404

**端口**: 31276
**测试入口**: `./test_client -g -r Gateway -i AllowedRoutes/Selector`

### 3. 组合场景测试 (`Combined/`)

测试多个约束条件的组合：

#### 3.1 Listener Hostname + AllowedRoutes

- ✅ Hostname 匹配 + Same Namespace → 允许
- ❌ Hostname 匹配 + Different Namespace → 拒绝
- ❌ Same Namespace + Hostname 不匹配 → 拒绝

#### 3.2 sectionName + Listener Hostname

- ✅ sectionName 匹配 + Hostname 匹配 → 允许
- ❌ sectionName 匹配 + Hostname 不匹配 → 拒绝

**端口**: 31230-31232
**测试入口**: `./test_client -g -r Gateway -i Combined`

## 边界场景说明

以下边界场景已在现有测试中隐含覆盖：

1. **parentRef namespace 默认值**: 
   - HTTPRoute 的 `parentRef` 不指定 `namespace` 时，默认使用 Route 的 namespace
   - 已在多个测试配置中验证（符合 Gateway API 规范）

2. **Route 无 hostnames**:
   - HTTPRoute 不指定 `hostnames` 字段时，Listener 的 hostname 约束仍然有效
   - 测试逻辑: Route 没有 hostname 约束时应允许匹配任何 Listener

## 运行测试

### 运行所有 Gateway 测试
```bash
cd examples/test
./scripts/integration/run_integration.sh -r Gateway
```

### 运行特定测试套件
```bash
# Listener Hostname 测试
./scripts/integration/run_integration.sh -r Gateway -i ListenerHostname

# AllowedRoutes Same 测试
./scripts/integration/run_integration.sh -r Gateway -i AllowedRoutes/Same

# AllowedRoutes All 测试
./scripts/integration/run_integration.sh -r Gateway -i AllowedRoutes/All

# AllowedRoutes Kinds 测试
./scripts/integration/run_integration.sh -r Gateway -i AllowedRoutes/Kinds

# AllowedRoutes Selector 测试
./scripts/integration/run_integration.sh -r Gateway -i AllowedRoutes/Selector

# Combined 测试
./scripts/integration/run_integration.sh -r Gateway -i Combined
```

## 配置注意事项

### 文件命名规范

为确保正确的加载顺序，Gateway 配置文件使用 `01_Gateway.yaml` 前缀命名：
- `01_Gateway.yaml` - 确保 Gateway 在 HTTPRoute 之前被加载
- `HTTPRoute_*.yaml` - HTTPRoute 配置文件

这样可以避免 HTTPRoute 处理时 Gateway 尚未加载完成的问题。

### 端口分配

各测试套件使用的端口范围：
- **ListenerHostname**: 31240-31242
- **AllowedRoutes/Same**: 31210
- **AllowedRoutes/All**: 31211
- **AllowedRoutes/Kinds**: 31213
- **Combined**: 31230-31232

注意：避免与现有测试端口冲突（如 EdgionTls 使用 31200）。

### 7. 动态配置测试 (`DynamicTest/`)

测试 Gateway 资源的动态更新能力（运行时配置变更）：

#### 测试场景

1. **Gateway Hostname 约束动态移除**
   - 初始：hostname 限制生效，非匹配 hostname 被拒绝（404）
   - 更新：移除 hostname 限制
   - 验证：之前被拒绝的 hostname 现在可以访问（200）

2. **AllowedRoutes 动态变更** (Same → All)
   - 初始：仅允许同 namespace，跨 namespace 被拒绝（404）
   - 更新：改为允许所有 namespace
   - 验证：跨 namespace 路由现在可以访问（200）

3. **HTTPRoute 动态添加**
   - 初始：新路由不存在（404）
   - 更新：添加新 HTTPRoute
   - 验证：新路由立即可访问（200）

4. **HTTPRoute Match 规则动态修改**
   - 初始：仅匹配 GET，POST 被拒绝（404）
   - 更新：改为匹配 POST
   - 验证：POST 请求现在可以访问（200）

5. **HTTPRoute 动态删除**
   - 初始：路由存在（200）
   - 更新：删除 HTTPRoute
   - 验证：路由不可访问（404）

**端口**: 31250-31252  
**测试入口**: `./test_client -g -r Gateway -i Dynamic --phase initial|update`  
**完整流程**: `./run_integration.sh -r Gateway --dynamic-test`

详见: [DynamicTest/TEST_SUMMARY.md](DynamicTest/TEST_SUMMARY.md)

---

## 测试覆盖总结

| 功能 | 测试场景 | 状态 |
|------|---------|------|
| Listener Hostname - 精确匹配 | ✅ 正面测试 + ❌ 负面测试 | ✅ |
| Listener Hostname - 通配符 | ✅ 正面测试 + ❌ 负面测试 | ✅ |
| Listener Hostname - 无限制 | ✅ 正面测试 | ✅ |
| AllowedRoutes - Same Namespace | ✅ 正面测试 + ❌ 负面测试 | ✅ |
| AllowedRoutes - All Namespaces | ✅ 正面测试（跨 namespace） | ✅ |
| AllowedRoutes - Selector | ✅ namespace label 正面测试 + ❌ 跨 namespace 负面测试 | ✅ |
| AllowedRoutes - Kinds | ✅ HTTPRoute 允许 + ❌ GRPCRoute 拒绝 | ✅ |
| 组合场景 - Hostname + AllowedRoutes | ✅ 正面测试 + ❌ 负面测试（2种） | ✅ |
| 组合场景 - sectionName + Hostname | ✅ 正面测试 + ❌ 负面测试 | ✅ |
| 边界场景 - parentRef 默认 namespace | ✅ 隐含覆盖 | ✅ |
| 边界场景 - Route 无 hostnames | ✅ 隐含覆盖 | ✅ |
| **动态配置 - Gateway Hostname 移除** | ✅ 404→200 状态转换 | ✅ |
| **动态配置 - AllowedRoutes 变更** | ✅ 404→200 状态转换 | ✅ |
| **动态配置 - HTTPRoute 增删改** | ✅ 3个场景（添加/修改/删除） | ✅ |

## 参考资料

- Kubernetes Gateway API Spec: https://gateway-api.sigs.k8s.io/
- 实现代码: `src/core/gateway/runtime/matching/route.rs` (`check_gateway_listener_match`)
- 测试代码: `examples/code/client/suites/gateway/`
