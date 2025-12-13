# Edgion 测试示例

本目录包含用于测试 Edgion 功能的示例程序。

## ✅ 验证状态

所有测试工具已验证可正常工作：
- ✅ **test_http_server** - HTTP 测试服务器（4个实例，端口 30001-30004）
- ✅ **test_websocket_server** - WebSocket 测试服务器（4个实例，端口 30011-30014）
- ✅ **test_websocket_client** - WebSocket 客户端（支持 ws:// 和 http:// 协议）

详细测试结果: [WEBSOCKET_TEST_RESULTS.md](WEBSOCKET_TEST_RESULTS.md)

## HTTP 测试服务器

**文件**: `test_http_server.rs`

启动 4 个 HTTP 测试服务器，用于测试 HTTP 代理和负载均衡功能。

```bash
# 运行 HTTP 测试服务器
cargo run --example test_http_server
```

**监听端口**:
- `http://127.0.0.1:30001`
- `http://127.0.0.1:30002`
- `http://127.0.0.1:30003`
- `http://127.0.0.1:30004`

**功能**:
- 返回请求信息（Host, Path, Client Address, Headers）
- 显示服务器地址以区分不同的后端

**测试示例**:
```bash
# 直接访问测试
curl http://127.0.0.1:30001/test
curl http://127.0.0.1:30002/api/users
```

---

## WebSocket 测试服务器

**文件**: `test_websocket_server.rs`

启动 4 个 WebSocket 测试服务器，用于测试 WebSocket 代理功能。

```bash
# 运行 WebSocket 测试服务器
cargo run --example test_websocket_server
```

**监听端口**:
- `ws://127.0.0.1:30011/ws`
- `ws://127.0.0.1:30012/ws`
- `ws://127.0.0.1:30013/ws`
- `ws://127.0.0.1:30014/ws`

**功能**:
- Echo 服务器：回显所有接收到的文本和二进制消息
- 自动响应 Ping/Pong
- 显示连接信息和消息日志
- 在回显消息中包含服务器地址以区分不同的后端

---

## WebSocket 测试客户端

**文件**: `test_websocket_client.rs`

用于测试 WebSocket 连接的客户端程序。

```bash
# 使用默认服务器 (ws://127.0.0.1:30011/ws)
cargo run --example test_websocket_client

# 指定服务器地址 (支持 ws:// 和 http:// 协议)
cargo run --example test_websocket_client ws://127.0.0.1:30012/ws
cargo run --example test_websocket_client http://127.0.0.1:30012/ws

# 通过 Edgion 代理测试
cargo run --example test_websocket_client ws://127.0.0.1:8080/ws
cargo run --example test_websocket_client http://127.0.0.1:8080/ws
```

**功能**:
- 连接到指定的 WebSocket 服务器
- **支持 `ws://` 和 `http://` 协议**（自动转换）
- 发送 5 条测试消息
- 发送 Ping 消息
- 发送二进制数据
- 接收并显示服务器响应
- 自动关闭连接

**已验证**:
- ✅ 直接连接到 test_websocket_server（ws:// 和 http:// 协议）
- ✅ 消息发送和接收
- ✅ Ping/Pong 支持
- ✅ 二进制数据传输

---

## 快速测试

### 测试 WebSocket 服务器和客户端连接

使用提供的测试脚本验证 WebSocket 功能：

```bash
# 运行自动化测试（包含 ws:// 和 http:// 协议测试）
./examples/test_ws_connection.sh
```

或者手动测试：

```bash
# Terminal 1: 启动 WebSocket 服务器
cargo run --example test_websocket_server

# Terminal 2: 测试连接
cargo run --example test_websocket_client ws://127.0.0.1:30011/ws
# 或使用 http:// 协议
cargo run --example test_websocket_client http://127.0.0.1:30011/ws
```

---

## 使用场景

### 1. 测试 HTTP 负载均衡

```bash
# Terminal 1: 启动 HTTP 测试服务器
cargo run --example test_http_server

# Terminal 2: 启动 Edgion（配置好 HTTPRoute 指向 30001-30004）
cargo run -- -c config.yaml

# Terminal 3: 测试负载均衡
for i in {1..10}; do curl http://127.0.0.1:8080/test; done
```

### 2. 测试 WebSocket 代理

```bash
# Terminal 1: 启动 WebSocket 测试服务器
cargo run --example test_websocket_server

# Terminal 2: 启动 Edgion（配置好 WebSocket 路由）
cargo run -- -c config.yaml

# Terminal 3: 测试 WebSocket 连接
cargo run --example test_websocket_client ws://127.0.0.1:8080/ws
```

### 3. 测试重试和故障转移

```bash
# Terminal 1: 启动部分服务器（模拟部分服务器故障）
cargo run --example test_http_server
# 然后手动停止某些服务器

# Terminal 2: 观察 Edgion 的重试行为
curl http://127.0.0.1:8080/test
```

---

## 端口分配

| 服务类型 | 端口范围 | 用途 |
|---------|---------|------|
| HTTP 测试服务器 | 30001-30004 | HTTP 负载均衡测试 |
| WebSocket 测试服务器 | 30011-30014 | WebSocket 代理测试 |
| Edgion 网关 | 8080 (默认) | 主代理入口 |

---

## 依赖说明

- **axum**: HTTP 和 WebSocket 服务器框架
- **tokio**: 异步运行时
- **tokio-tungstenite**: WebSocket 客户端库（dev dependency）

所有依赖已在 `Cargo.toml` 中配置好，无需额外安装。
