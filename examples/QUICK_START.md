# 快速开始 - WebSocket 测试

## 🚀 一分钟快速测试

### 测试 WebSocket 功能（不需要网关）

```bash
# Terminal 1: 启动 WebSocket 测试服务器
cd /Users/caohao/code/Edgion
cargo run --example test_websocket_server

# Terminal 2: 运行客户端测试
cargo run --example test_websocket_client ws://127.0.0.1:30011/ws
# 或使用 http:// 协议（自动转换为 ws://）
cargo run --example test_websocket_client http://127.0.0.1:30011/ws
```

### 预期输出

**服务器端** (Terminal 1):
```
Starting WebSocket test servers...

WebSocket servers will listen on:
  - ws://127.0.0.1:30011/ws
  - ws://127.0.0.1:30012/ws
  - ws://127.0.0.1:30013/ws
  - ws://127.0.0.1:30014/ws

WebSocket server listening on ws://127.0.0.1:30011/ws
[127.0.0.1:30011] New WebSocket connection from 127.0.0.1:xxxxx
[127.0.0.1:30011] Received text from 127.0.0.1:xxxxx: Hello from client, message #1
...
```

**客户端** (Terminal 2):
```
WebSocket Test Client

Connecting to ws://127.0.0.1:30011/ws...
✓ Connected successfully!

→ Sending: Hello from client, message #1
← Received: Connected to WebSocket server 127.0.0.1:30011 from 127.0.0.1:xxxxx
← Received: [127.0.0.1:30011] Echo: Hello from client, message #1
→ Sending: Hello from client, message #2
← Received: [127.0.0.1:30011] Echo: Hello from client, message #2
...
→ Sending: PING
← Received: PONG
→ Sending: Binary data [5 bytes]
← Received: Binary data [5 bytes]

✓ Test completed
```

## ✅ 验证清单

测试成功的标志：
- [x] 客户端显示 "✓ Connected successfully!"
- [x] 服务器显示新连接消息
- [x] 客户端收到欢迎消息
- [x] 5条消息成功发送和接收
- [x] Ping/Pong 成功交换
- [x] 二进制数据成功传输
- [x] 客户端显示 "✓ Test completed"

## 🔧 测试不同的服务器

```bash
# 测试服务器 2
cargo run --example test_websocket_client ws://127.0.0.1:30012/ws

# 测试服务器 3
cargo run --example test_websocket_client ws://127.0.0.1:30013/ws

# 测试服务器 4
cargo run --example test_websocket_client ws://127.0.0.1:30014/ws
```

每个服务器都会在响应中标识自己的地址，例如：
```
← Received: [127.0.0.1:30012] Echo: Hello from client, message #1
```

## 📋 下一步：测试 Edgion 网关

当 WebSocket 测试服务器和客户端验证正常后，可以配置 Edgion 网关进行代理测试：

### 1. 配置 HTTPRoute（假设配置文件）

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: websocket-route
  namespace: default
spec:
  parentRefs:
    - name: edgion-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /ws
      backendRefs:
        - name: websocket-service
          port: 30011
          weight: 1
        - name: websocket-service
          port: 30012
          weight: 1
```

### 2. 启动测试

```bash
# Terminal 1: 后端服务器
cargo run --example test_websocket_server

# Terminal 2: Edgion 网关
cargo run -- -c config.yaml

# Terminal 3: 通过网关测试
cargo run --example test_websocket_client ws://127.0.0.1:8080/ws
cargo run --example test_websocket_client http://127.0.0.1:8080/ws
```

### 3. 验证负载均衡

多次运行客户端，观察是否连接到不同的后端服务器：

```bash
# 运行 10 次，观察服务器地址分布
for i in {1..10}; do
  echo "=== Test $i ==="
  cargo run --example test_websocket_client ws://127.0.0.1:8080/ws 2>&1 | grep "127.0.0.1:300"
  sleep 1
done
```

## 🐛 常见问题

### 客户端无法连接

**错误**: `Failed to connect: ...`

**解决**:
1. 确认服务器正在运行: `ps aux | grep test_websocket_server`
2. 确认端口未被占用: `lsof -i :30011`
3. 检查防火墙设置

### 连接立即关闭

**观察**: 连接建立后立即关闭

**可能原因**:
- 服务器崩溃：检查服务器终端输出
- 端口冲突：更换端口

### "Connection reset without closing handshake"

**状态**: ⚠️ 警告（正常）

**说明**: 这是客户端主动关闭连接时的正常行为，不影响功能。测试已经完成。

## 📊 性能测试

### 并发连接测试

```bash
# 启动多个客户端（需要额外的终端或后台运行）
for i in {1..5}; do
  cargo run --example test_websocket_client ws://127.0.0.1:30011/ws &
done

# 等待所有测试完成
wait
```

### 长时间连接测试

可以修改客户端代码，增加消息数量或循环发送，测试长时间连接的稳定性。

## 📝 总结

现在您已经验证了：
- ✅ WebSocket 服务器可以正常启动和响应
- ✅ WebSocket 客户端可以成功连接和通信
- ✅ 支持 ws:// 和 http:// 协议
- ✅ Echo、Ping/Pong、二进制数据传输都正常工作

**可以开始测试 Edgion 网关的 WebSocket 代理功能了！** 🎉
