# Gateway 动态配置测试总结

## 测试目标

验证 Gateway 和 HTTPRoute 的动态配置更新功能，证明配置可以在运行时动态修改并立即生效。

## 测试场景

### 场景 1: Gateway Hostname 动态移除

**目标**: 验证 Gateway Listener 的 hostname 约束可以动态移除

- **初始状态**: Listener `http-with-hostname` (31250) 有 `hostname: api.example.com` 限制
- **初始验证**: 访问 `other.example.com` 被拒绝 (404)
- **动态更新**: 通过 API 移除 hostname 字段
- **更新后验证**: 访问 `other.example.com` 现在成功 (200/502)

**验证点**: Gateway 配置的动态性（hostname 约束）

### 场景 2: HTTPRoute 方法动态修改

**目标**: 验证 HTTPRoute 的匹配规则可以动态修改

- **初始状态**: HTTPRoute 匹配 `GET /api/v1`
- **初始验证**: GET 请求成功 (200/502)
- **动态更新**: 通过 API 修改为匹配 `POST /api/v1`
- **更新后验证**: GET 失败 (404)，POST 成功 (200/502)

**验证点**: HTTPRoute 配置的动态性（方法匹配）

## 配置文件结构

```
DynamicTest/
├── initial/              # 初始配置
│   ├── 01_Gateway.yaml                 # Gateway: 2 个 Listener
│   ├── HTTPRoute_hostname_match.yaml   # 用于 hostname 测试
│   └── HTTPRoute_method.yaml           # 匹配 GET /api/v1
└── updates/              # 动态更新配置
    ├── Gateway_remove_hostname.yaml    # 移除 hostname
    └── HTTPRoute_method_update.yaml    # 修改为 POST /api/v1
```

## 测试流程

1. **启动服务**: Gateway + Controller
2. **加载初始配置**: 通过 API 加载 `initial/` 目录
3. **初始阶段测试**: 
   - ✓ Hostname 限制生效 (other.example.com → 404)
   - ✓ GET 方法匹配 (GET /api/v1 → 200/502)
4. **动态更新**: 通过 `edgion-ctl apply` 加载 `updates/` 目录
5. **等待生效**: 2 秒
6. **更新阶段测试**:
   - ✓ Hostname 限制移除 (other.example.com → 200/502)
   - ✓ 方法修改生效 (GET → 404, POST → 200/502)
7. **资源同步验证**: resource_diff

## 运行测试

```bash
cd /Users/caohao/ws1/Edgion
./examples/test/scripts/integration/run_integration.sh -r Gateway --dynamic-test
```

## 预期结果

```
[✓] Initial Phase Tests (2/2 passed)
[✓] Dynamic Update Applied
[✓] Update Phase Tests (2/2 passed)
[✓] Resource Sync Verified
```

## 技术细节

### Gateway 动态性

- **静态部分**: Listener 端口和协议（需要重启）
- **动态部分**: hostname、allowedRoutes 等配置（ArcSwap 实现）

### HTTPRoute 动态性

- **完全动态**: 所有配置都可以动态更新
- **生效时间**: 立即生效（< 1 秒）

### 实现机制

1. `GatewayConfigStore` - 全局配置存储（ArcSwap）
2. `ConfHandler<Gateway>` - 监听配置变更事件
3. Controller API - 支持 Gateway/HTTPRoute 的创建和更新

## 关键文件

- `/src/core/gateway/gateway/config_store.rs` - 配置存储
- `/src/core/gateway/gateway/handler.rs` - Gateway 处理器
- `/src/core/api/controller/namespaced_handlers.rs` - Controller API
- `/examples/test/scripts/utils/load_conf.sh` - 配置加载脚本
