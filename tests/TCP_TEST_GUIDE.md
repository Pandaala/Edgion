# TCP 路由测试指南

本指南帮助你测试 Edgion Gateway 的 TCPRoute 功能。

## 📋 测试环境准备

### 1. 后端服务器 (test_http_server)

后端服务器已存在，提供 4 个 HTTP 端口：30001-30004

启动后端服务器：
```bash
cargo run --example test_http_server
```

你应该看到类似输出：
```
Starting test servers...
Listening on 127.0.0.1:30001
Listening on 127.0.0.1:30002
Listening on 127.0.0.1:30003
Listening on 127.0.0.1:30004
```

### 2. 配置文件

已创建以下测试配置：

#### Gateway 配置 (`config/examples/Gateway_default_gateway1.yaml`)
```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: gateway1
  namespace: default
spec:
  gatewayClassName: public-gateway
  listeners:
    - name: https18443
      protocol: HTTPS
      port: 18443
    - name: http18080
      protocol: HTTP
      port: 18080
    - name: tcp19000        # ← TCP 监听器
      protocol: TCP
      port: 19000
```

#### TCPRoute 配置 (`config/examples/TCPRoute_test1_tcp-route1.yaml`)
```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: tcp-route1
  namespace: test1
spec:
  parentRefs:
    - name: gateway1
      namespace: default
      sectionName: tcp19000
  rules:
    - backendRefs:
        - name: aaa-service
          namespace: test1
          port: 30001
          weight: 100
```

#### 后端服务配置（已存在）
- `Service_test1_aaa-service.yaml` - 定义服务
- `EndpointSlice_test1_aaa-service.yaml` - 定义端点（包含 127.0.0.1:30001）

## 🚀 启动测试

### 步骤 1: 启动后端服务器
```bash
# 终端 1
cd /Users/caohao/code/Edgion
cargo run --example test_http_server
```

### 步骤 2: 启动 Edgion Gateway
```bash
# 终端 2
cd /Users/caohao/code/Edgion
cargo run --bin edgion-gateway
```

### 步骤 3: 加载配置

确保以下配置已加载到你的 ConfigServer 或 K8s 集群：

```bash
# 如果使用文件加载模式，确保配置在正确目录
ls -la config/examples/ | grep -E "(Gateway|TCPRoute|Service|EndpointSlice)_.*\.yaml"
```

需要的配置文件：
- ✅ `Gateway_default_gateway1.yaml`
- ✅ `TCPRoute_test1_tcp-route1.yaml`
- ✅ `Service_test1_aaa-service.yaml`
- ✅ `EndpointSlice_test1_aaa-service.yaml`
- ✅ `GatewayClass__public-gateway.yaml`
- ✅ `EdgionGatewayConfig__public-gateway.yaml`

### 步骤 4: 测试 TCP 连接

#### 方法 1: 使用提供的测试脚本
```bash
# 终端 3
./test_tcp_client.sh 19000 127.0.0.1
```

#### 方法 2: 使用 nc (netcat)
```bash
# 发送 HTTP 请求到 TCP 端口
echo -e "GET / HTTP/1.1\r\nHost: test.example.com\r\n\r\n" | nc 127.0.0.1 19000
```

#### 方法 3: 使用 telnet
```bash
telnet 127.0.0.1 19000
# 然后手动输入：
# GET / HTTP/1.1
# Host: test.example.com
# (按两次回车)
```

#### 方法 4: 使用 curl (通过 TCP 代理)
```bash
curl http://127.0.0.1:19000/ -H "Host: test.example.com"
```

## ✅ 预期结果

如果一切正常，你应该看到来自后端服务器 (127.0.0.1:30001) 的响应：

```
============= Response from 127.0.0.1:30001 ===========
Host: test.example.com
Path: /
Client Address: 127.0.0.1
Client Port: xxxxx

Headers:
  host: test.example.com
  ...
```

## 🔍 调试和日志

### 查看 Gateway 日志
Gateway 启动时应该显示：
```
INFO edgion::core::gateway::listener_builder: Adding TCP listener
     port = 19000
     gateway = "default/gateway1"
     
INFO edgion::core::gateway::listener_builder: TCP listener added successfully
     port = 19000
```

### 查看 TCPRoute 同步日志
```
DEBUG edgion::core::routes::tcp_routes: TCPRoute configuration updated
      route_key = "test1/tcp-route1"
      
DEBUG edgion::core::routes::tcp_routes: Rebuilt gateway TCP routes
      gateway = "default/gateway1"
      ports = [19000]
```

### 查看连接日志
当客户端连接时：
```
INFO edgion::core::routes::tcp_routes::edgion_tcp: TCP connection established
     port = 19000
     upstream = "aaa-service:30001"
     
DEBUG edgion::core::routes::tcp_routes::edgion_tcp: Client closed connection
```

### 查看访问日志
检查 `logs/edgion_access.log`：
```json
{
  "ts": 1734441234567,
  "protocol": "TCP",
  "listener_port": 19000,
  "client_addr": "unknown",
  "upstream_addr": "aaa-service:30001",
  "duration_ms": 123,
  "bytes_sent": 1024,
  "bytes_received": 2048,
  "status": "Success"
}
```

## 🐛 常见问题

### 1. 连接被拒绝
```
Connection refused
```
**原因**: Gateway 可能未启动或端口 19000 未监听
**解决**: 检查 Gateway 日志，确认 TCP listener 已成功添加

### 2. 连接超时
```
Connection timed out
```
**原因**: 没有匹配的 TCPRoute 或后端服务未运行
**解决**: 
- 检查 TCPRoute 是否已加载
- 检查后端服务器 (test_http_server) 是否运行在 30001 端口

### 3. No TCPRoute found for port
```
WARN No TCPRoute found for port
     port = 19000
     gateway = "default/gateway1"
```
**原因**: TCPRoute 配置未加载或不匹配
**解决**: 
- 确认 `TCPRoute_test1_tcp-route1.yaml` 已加载
- 确认 `parentRefs` 正确引用 `gateway1` 和 `sectionName: tcp19000`

### 4. Failed to connect to upstream
```
ERROR Failed to connect to upstream
      upstream = "aaa-service:30001"
```
**原因**: 
- 后端服务器未运行
- Service/EndpointSlice 配置问题
**解决**: 
- 启动 `test_http_server`
- 检查 Service 和 EndpointSlice 配置

## 📊 测试检查清单

- [ ] 后端服务器运行在 30001 端口
- [ ] Gateway 启动成功
- [ ] Gateway 监听 19000 端口
- [ ] GatewayClass 配置已加载
- [ ] Gateway 配置已加载（包含 TCP listener）
- [ ] Service 配置已加载
- [ ] EndpointSlice 配置已加载
- [ ] TCPRoute 配置已加载
- [ ] TCPRoute handler 已注册（已确认 ✅）
- [ ] 可以通过 nc/telnet/curl 连接 19000 端口
- [ ] 收到来自 30001 的响应
- [ ] 访问日志正确记录 TCP 连接

## 🎯 验证功能点

测试以下 TCPRoute 功能：

### 基础功能
- [x] TCP listener 创建
- [x] TCPRoute 配置同步
- [x] 端口匹配
- [x] 后端选择
- [x] TCP 双向转发
- [x] 访问日志记录

### 高级功能（后续测试）
- [ ] 多个 backendRefs（负载均衡）
- [ ] 不同权重分配
- [ ] 连接失败处理
- [ ] 上游断开处理
- [ ] 下游断开处理

## 📝 测试记录

记录你的测试结果：

| 测试项 | 状态 | 备注 |
|--------|------|------|
| 后端服务启动 | ⬜ | |
| Gateway 启动 | ⬜ | |
| TCP listener 创建 | ⬜ | |
| TCPRoute 加载 | ⬜ | |
| TCP 连接成功 | ⬜ | |
| 数据转发正常 | ⬜ | |
| 访问日志生成 | ⬜ | |

祝测试顺利！🚀

