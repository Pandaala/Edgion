# 06 - Traffic Split / Canary (流量分割 / 金丝雀发布)

**优先级**: P1 - 竞品标配

## 问题

灰度发布是现代部署的标配能力。Edgion 缺少按权重、header、cookie 将流量分发到不同 upstream 版本的能力，无法支撑金丝雀发布和 A/B 测试场景。

## 竞品对比

| 网关 | 实现方式 |
|------|---------|
| APISIX | `traffic-split` 插件，按权重分流到多个 upstream，支持条件匹配 |
| Kong | Canary Release 插件（Enterprise），按百分比逐步切换 |
| Envoy | weighted clusters + header/cookie-based 分流 |

## 需要实现

- [ ] 按权重比例分流到多个 upstream（如 90% v1 / 10% v2）
- [ ] 基于 header 的分流规则（如 X-Canary: true → v2）
- [ ] 基于 cookie 的分流规则
- [ ] 会话粘性（同一用户始终路由到同一版本）
- [ ] 动态调整权重（通过 API 或配置热更新）
- [ ] 作为 HTTP 插件实现

## 备注

Gateway API 的 HTTPRoute 本身支持 `backendRefs` 的 `weight` 字段，可能已有部分基础，需调研现有实现。
