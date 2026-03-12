# EdgionTls 端口隔离 — 集成测试影响分析

> 分析现有集成测试配置是否受本次改造影响，以及是否需要新增测试。

## 1. 现有 EdgionTls 测试配置审查

### 1.1 `examples/test/conf/` 下的 EdgionTls YAML（共 12 个）

| # | 文件路径 | EdgionTls 名称 | Gateway | sectionName | parentRefs |
|---|---------|---------------|---------|-------------|-----------|
| 1 | `EdgionTls/cipher/EdgionTls_cipher_modern.yaml` | cipher-modern | cipher-gateway | https-modern | ✅ 有 |
| 2 | `EdgionTls/cipher/EdgionTls_cipher_legacy.yaml` | cipher-legacy | cipher-gateway | https-legacy | ✅ 有 |
| 3 | `EdgionTls/https/EdgionTls.yaml` | edgiontls-https | edgiontls-https-gateway | https | ✅ 有 |
| 4 | `EdgionTls/grpctls/EdgionTls.yaml` | edgiontls-grpctls | edgiontls-grpctls-gateway | grpctls | ✅ 有 |
| 5 | `EdgionTls/mTLS/EdgionTls_edge_mtls-test-san.yaml` | mtls-test-san | mtls-test-gateway | https-mtls | ✅ 有 |
| 6 | `EdgionTls/mTLS/EdgionTls_edge_mtls-test-chain.yaml` | mtls-test-chain | mtls-test-gateway | https-mtls | ✅ 有 |
| 7 | `EdgionTls/mTLS/EdgionTls_edge_mtls-test-mutual.yaml` | mtls-test-mutual | mtls-test-gateway | https-mtls | ✅ 有 |
| 8 | `EdgionTls/mTLS/EdgionTls_edge_mtls-test-optional.yaml` | mtls-test-optional | mtls-test-gateway | https-mtls | ✅ 有 |
| 9 | `TLSRoute/Basic/02_EdgionTls.yaml` | tls-route-basic-cert | tls-route-basic-gw | tls-basic | ✅ 有 |
| 10 | `TLSRoute/ProxyProtocol/02_EdgionTls.yaml` | tls-pp2-gw-pp2-cert | tls-route-pp2-gw | tls-pp2 | ✅ 有 |
| 11 | `TLSRoute/StreamPlugins/03b_EdgionTls_allow.yaml` | tls-sp-allow-cert | tls-stream-plugins-gw | tls-allow-localhost | ✅ 有 |
| 12 | `TLSRoute/StreamPlugins/03c_EdgionTls_deny.yaml` | tls-sp-deny-cert | tls-stream-plugins-gw | tls-deny-all | ✅ 有 |

### 1.2 `examples/k8stest/conf/` 下的 EdgionTls YAML

与 `test/conf/` 完全镜像，同样全部包含 `parentRefs` + `sectionName`。

### 1.3 其他目录检查

| 目录 | EdgionTls 文件 |
|------|---------------|
| `conf/base/` | 无 |
| `conf/EdgionPlugins/` | 无 |
| `conf/HTTPRoute/` | 无 |
| `conf/Gateway/` | 无 |
| `conf/GRPCRoute/` | 无 |
| `conf/TCPRoute/` | 无 |
| `conf/UDPRoute/` | 无 |

## 2. 结论：现有测试无需修改

**所有 12 个 EdgionTls 测试配置都已经包含 `parentRefs` 和 `sectionName`。**

改造后的行为变化：

| 改造前 | 改造后 |
|-------|-------|
| controller 不解析 parentRef → port | controller 通过 sectionName → Gateway → listener.port 解析出端口，填入 `resolved_ports` |
| cert_matcher 全局 hostname 匹配 | cert_matcher 先按 port 匹配，再 fallback 到 global |
| 证书对所有端口可见 | 证书仅对 resolved_ports 中的端口可见 |

**为什么现有测试仍能通过：**

每个 EdgionTls 的 `sectionName` 都精确指向一个 Gateway listener。改造后 controller 会解析
出该 listener 的端口并填入 `resolved_ports`。由于测试中每个 hostname 只在一个端口上使用，
port-specific 匹配的结果与改造前的全局匹配完全一致。

### 端口映射验证

| EdgionTls | sectionName | Gateway listener port | 行为 |
|-----------|-------------|-----------------------|------|
| cipher-modern | https-modern | 31196 | ✅ 改造后 resolved_ports=[31196]，仅在 31196 匹配 |
| cipher-legacy | https-legacy | 31195 | ✅ 改造后 resolved_ports=[31195]，仅在 31195 匹配 |
| edgiontls-https | https | 31190 | ✅ 改造后 resolved_ports=[31190] |
| edgiontls-grpctls | grpctls | 31200 | ✅ 改造后 resolved_ports=[31200] |
| mtls-test-* (4个) | https-mtls | 31110 | ✅ 改造后 resolved_ports=[31110] |
| tls-route-basic-cert | tls-basic | 31280 | ✅ 改造后 resolved_ports=[31280] |
| tls-pp2-gw-pp2-cert | tls-pp2 | 31281 | ✅ 改造后 resolved_ports=[31281] |
| tls-sp-allow-cert | tls-allow-localhost | 31282 | ✅ 改造后 resolved_ports=[31282] |
| tls-sp-deny-cert | tls-deny-all | 31283 | ✅ 改造后 resolved_ports=[31283] |

## 3. 建议新增的集成测试（P1）

### 3.1 端口隔离测试

**目的**：验证改造的核心功能——不同端口的 EdgionTls 证书不会交叉匹配。

**方案**：创建两个 Gateway listener 监听不同端口，配置相同 hostname 但不同的 EdgionTls
（通过 minTlsVersion 差异来区分），验证每个端口使用正确的配置。

**测试目录**：`conf/EdgionTls/PortIsolation/`

需要的配置：

```
conf/EdgionTls/PortIsolation/
├── Gateway.yaml              # 两个 TLS listener: port-a (31284), port-b (31285)
├── EdgionTls_port_a.yaml     # parentRef → sectionName: listener-a → minTlsVersion: TLS1_2
├── EdgionTls_port_b.yaml     # parentRef → sectionName: listener-b → minTlsVersion: TLS1_3
├── HTTPRoute_port_a.yaml     # 路由到 test-http
└── HTTPRoute_port_b.yaml     # 路由到 test-http
```

测试代码验证：
- 端口 31284 接受 TLS 1.2 连接（使用 EdgionTls A 的 minTlsVersion: TLS1_2）
- 端口 31285 拒绝 TLS 1.2 连接（使用 EdgionTls B 的 minTlsVersion: TLS1_3）
- 两者使用相同 hostname，证明隔离生效

### 3.2 全局 fallback 测试

**目的**：验证无 parentRefs 的 EdgionTls 仍然对所有端口生效（向后兼容）。

**方案**：创建一个不带 parentRefs 的 EdgionTls，验证它在多个端口上都能匹配。

> 注意：这两个测试属于 P1 优先级，在核心改造验证完毕后实施。

## 4. 风险评估

| 风险 | 影响 | 概率 | 缓解措施 |
|------|------|------|---------|
| Gateway 尚未到达时 resolved_ports=None | 证书暂时全局可见，等 Gateway 到达后 requeue 修正 | 低（启动顺序通常 Gateway 先到） | 可接受，与 Secret 未到达的处理模式一致 |
| lookup_gateway O(n) 性能 | parse() 慢 | 极低（Gateway 数量通常 < 10） | 后续 ProcessorObj 暴露 get(key) 后优化 |
| 现有测试因 port 隔离而失败 | 需要修复测试 | 无（已验证所有测试都有正确的 parentRefs） | N/A |
