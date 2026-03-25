# UDPRoute Conformance Tests (1 test)

### 1. UDPRoute
- **Features**: UDPRoute, Gateway
- **Manifest**: `udproute-simple.yaml`
- **描述**: 基本 UDP 路由
- **测试逻辑**:
  1. Gateway 配置 UDP listener
  2. 创建 UDPRoute，绑定到 UDP listener
  3. 发送 UDP 数据包到 Gateway
  4. 验证 UDP 流量被正确转发到后端 Service
  5. 验证后端收到原始 UDP payload
  6. 验证 UDPRoute status 正确 (Accepted, ResolvedRefs)
