# 状态条件与上报完善

优先级: P0/P1

## 1. Programmed 条件 [P0]

### 现状

- `status_utils.rs:9-12` 注释明确说明：因 Controller-Gateway 异步 gRPC sync 架构，无数据面状态回传通道，故意省略 Programmed 和 Ready 条件
- Gateway API 规范将 `Programmed` 列为 Core 必需条件

### 建议方案

规范允许在配置**发送到**数据面后设置 `Programmed: True`（不要求数据面确认生效）。方案：

1. gRPC ConfigSync `Watch`/`List` 推送成功后，Controller 标记对应资源 `Programmed: True`
2. 若无 gateway 连接或推送失败，设置 `Programmed: False`，reason `NoConnectedDataPlane`
3. Gateway 级别和 Listener 级别均需设置

### 涉及文件

- `src/core/controller/sync_runtime/resource_processor/handlers/gateway.rs`
- `src/core/controller/sync_runtime/status_utils.rs`
- `src/core/controller/conf_sync/` — 推送结果回调

---

## 2. GatewayClass.status.supportedFeatures [P1]

### 现状

- `GatewayClassStatus` 只有 `conditions: Vec<Condition>`
- conformance test 已声明支持的特性列表（HTTPRoute、ReferenceGrant、query param matching、method matching 等）
- 该列表未反映到 CRD status

### 需要实现

1. 在 `GatewayClassStatus` 添加 `supported_features: Option<Vec<String>>` 字段
2. 在 `gateway_class.rs` handler 的 `parse()` 中填充已知支持特性
3. 特性列表应与 conformance test 声明保持一致

### 涉及文件

- `src/types/resources/gateway_class.rs`
- `src/core/controller/sync_runtime/resource_processor/handlers/gateway_class.rs`

---

## 3. PartiallyInvalid 条件 [P1]

### 现状

- Route 验证是全有或全无：所有规则有效则 Accepted，任一无效则整体 rejected
- Gateway API 规范要求：部分规则有效时设置 `PartiallyInvalid` 条件，有效规则继续生效

### 需要实现

1. 定义 `PartiallyInvalid` 条件类型
2. 逐规则验证，收集有效/无效规则
3. 若部分有效：设置 `Accepted: True` + `PartiallyInvalid: True`，message 中列出无效规则
4. 仅转发有效规则到数据面

### 涉及文件

- `src/types/resources/common.rs` — 条件类型
- `src/core/controller/sync_runtime/resource_processor/handlers/http_route.rs`
- `src/core/controller/sync_runtime/resource_processor/handlers/grpc_route.rs`

---

## 4. Gateway addresses 状态 [P2]

### 现状

- 从 spec.addresses、Service ClusterIP、默认配置、fallback `0.0.0.0` 推导
- 不反映数据面实际绑定地址（受限于无状态回传通道）

### 长期方案

- 若实现数据面状态回传（为 Programmed 条件），可顺带上报实际绑定地址
