# HTTPRoute Conformance Fixes

Gateway API v1.5 conformance 测试结果 (2026-03-28)

- **PASS**: 31
- **FAIL**: 11 (需修复，分 6 组)
- **SKIP**: 9 (含 2 个因不支持动态端口已标记 SKIP)

## 分组索引

| 组 | 文件 | 测试数 | 根因 | 优先级 |
|----|------|--------|------|--------|
| A | [A-status-conditions.md](httproute-conformance-fixes/A-status-conditions.md) | 6 | Controller backendRef 校验 + status 回写缺失 | 优先修 |
| B | [B-attached-routes.md](httproute-conformance-fixes/B-attached-routes.md) | 1 | attachedRoutes 未按 hostname 过滤 | 紧随 A |
| C | [C-https-listener.md](httproute-conformance-fixes/C-https-listener.md) | 1 | 数据面 HTTPS listener hostname 过滤缺失 | 独立 |
| D | [D-reference-grant.md](httproute-conformance-fixes/D-reference-grant.md) | 1 | ReferenceGrant 删除后路由消失而非返回 500 | 依赖 A |
| E | [E-timeout.md](httproute-conformance-fixes/E-timeout.md) | 2 | timeout 未生效 + "0s" 语义错误 | 独立 |
| F | [F-percentage-mirror.md](httproute-conformance-fixes/F-percentage-mirror.md) | 1 | Fraction 字段未实现 | 独立 |
