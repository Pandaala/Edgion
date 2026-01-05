# Edgion 隐藏逻辑文档

> 记录代码中的隐式行为和特殊处理点

**最后更新**: 2025-11-30

---

## #1 BackendRef.kind 字段处理

**配置**: `HTTPRoute.spec.rules[].backendRefs[].kind`

**特殊处理**:
- `kind` 为 `None`、空字符串或未识别值时，默认使用 `Service`
- 只有 `Service`、`ServiceClusterIp`、`ServiceExternalName` 被处理
- `ServiceImport` 类型目前会返回错误

**代码位置**: `edgion/src/core/backends/mod.rs` (get_peer 函数)

---

## #2 BackendRef.weight 字段默认值

**配置**: `HTTPRoute.spec.rules[].backendRefs[].weight`

**特殊处理**:
- 未指定 `weight` 时，默认值为 `1`
- `weight: 0` 的后端不接收流量（用于蓝绿/金丝雀发布）

**代码位置**: 
- `edgion/src/core/routes/match_engine/regex_routes_engine.rs:56`
- `edgion/src/core/routes/routes_mgr.rs:53`

---

## #3 BackendRef.namespace 优先级

**配置**: `HTTPRoute.spec.rules[].backendRefs[].namespace`

**特殊处理**:
- 优先使用 `br.namespace`，未指定时才使用 HTTPRoute 所在的 namespace
- 允许跨 namespace 引用后端服务

**代码位置**: `edgion/src/core/backends/mod.rs` (get_peer 函数)

---

## #4 负载均衡算法配置

**配置**: `HTTPRoute.spec.rules[].filters[].extensionRef.name` 直接指定算法

**特殊处理**:
- 默认使用 RoundRobin 算法
- 可通过 `extensionRef.name` 直接指定 Ketama/FnvHash/LeastConnection 算法
- 算法配置自动应用到同一 rule 的所有 backendRefs
- 算法按需懒加载，未使用时仅占用 24 字节
- 支持引用计数，自动清理未使用的策略

**示例配置**:
```yaml
rules:
  - filters:
      # 直接在 name 中指定算法，逗号分隔
      - type: ExtensionRef
        extensionRef:
          name: ketama,fnvhash
    backendRefs:
      - name: my-service
        port: 8080
      - name: api-service
        port: 9090
```

**代码位置**:
- 策略存储: `edgion/src/core/lb/optional_lb/policy_store.rs`
- 策略提取: `edgion/src/core/routes/conf_handler_impl.rs::extract_and_update_lb_policies()`
- ConfHandler 集成: `edgion/src/core/routes/conf_handler_impl.rs::ConfHandler<HTTPRoute>`
- 使用文档: `edgion/docs/lb-policy-usage.md`

**状态**: ✅ 已完成实现

---

## #5 Service Update 并发控制

**场景**: PolicyStore 变更和 EndpointSlice 更新同时发生

**特殊处理**:
- 使用服务级别的锁（每个 service 一把锁）
- 防止 LoadBalancer 状态不一致
- 锁通过 `DashMap<String, Arc<Mutex<()>>>` 管理

**代码位置**: `edgion/src/core/backends/service_update_lock.rs` (待实现)

**关键函数**:
- `get_service_update_lock(service_key: &str)` - 获取服务锁
- `update_in_place_and_refresh_lb()` - 需要先加锁
- `refresh_service_optional_lbs()` - 需要先加锁

