# Edgion 测试脚本

本目录包含 Edgion 项目的各类测试和构建脚本。

## 目录结构

```
scripts/
├── ci/                 # CI/CD 相关脚本
│   └── check.sh        # fmt/clippy/单元测试检查
├── integration/        # 集成测试脚本
│   ├── run_direct.sh   # 直接测试（不通过 Gateway）
│   └── run_integration.sh  # 集成测试（通过 Gateway）
└── utils/              # 工具脚本
    ├── prepare.sh      # 预编译测试组件
    ├── start_all.sh    # 启动所有测试服务
    └── kill_all.sh     # 停止所有测试服务
```

## CI 脚本

### check.sh

运行代码质量检查（格式、lint、单元测试）。

```bash
# 运行所有检查
./scripts/ci/check.sh

# 只检查格式
./scripts/ci/check.sh -f

# 只运行 clippy
./scripts/ci/check.sh -c

# 只运行单元测试
./scripts/ci/check.sh -t

# 自动修复问题
./scripts/ci/check.sh --fix

# 显示详细输出
./scripts/ci/check.sh -v
```

## 集成测试脚本

### run_direct.sh

直接测试 test_client 与 test_server 的连通性（不通过 Gateway）。

```bash
./scripts/integration/run_direct.sh
```

测试项：
- `http` - HTTP 基础连接
- `grpc` - gRPC 基础连接
- `websocket` - WebSocket 连接
- `tcp` - TCP 连接
- `udp` - UDP 连接

### run_integration.sh

通过 Gateway 进行完整链路集成测试。

```bash
# 运行所有集成测试
./scripts/integration/run_integration.sh

# 运行指定测试
./scripts/integration/run_integration.sh --test http-match

# 跳过某些测试
./scripts/integration/run_integration.sh --skip "mtls,backend-tls"
```

测试项：
- 基础协议：`http`, `https`, `grpc`, `grpc-tls`, `websocket`, `tcp`, `udp`
- 路由匹配：`http-match`, `grpc-match`
- HTTP 功能：`http-redirect`, `http-security`
- TLS：`mtls`, `backend-tls`
- 负载均衡：`lb-rr` (RoundRobin), `lb-ch` (ConsistentHash), `weighted-backend`
- 高级功能：`timeout`, `real-ip`, `security`, `plugin-logs`

## 工具脚本

### prepare.sh

预编译所有测试所需的组件（debug 模式）。

```bash
# 编译所有组件
./scripts/utils/prepare.sh
```

编译的组件：
- `edgion-controller` - 配置控制器
- `edgion-gateway` - 网关服务
- `edgion-ctl` - 命令行工具
- `test_server` - 测试后端服务器
- `test_client` - 集成测试客户端
- `test_client_direct` - 直接测试客户端

编译产物位置：
```
target/debug/
├── edgion-controller
├── edgion-gateway
├── edgion-ctl
└── examples/
    ├── test_server
    └── test_client
```

### start_all.sh

启动所有测试服务（test_server、controller、gateway）。

```bash
# 启动所有服务
./scripts/utils/start_all.sh
```

启动的服务：
- `test_server` - 测试后端服务器（HTTP/gRPC/WebSocket/TCP/UDP）
- `edgion-controller` - 配置控制器
- `edgion-gateway` - 网关服务

### kill_all.sh

停止所有 Edgion 相关进程。

```bash
# 停止所有服务
./scripts/utils/kill_all.sh
```
