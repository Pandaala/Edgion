# Edgion 集成测试配置

本目录包含集成测试所需的 Kubernetes Gateway API 配置文件。

## 目录结构

```
conf/
├── base/                   # 基础配置（所有测试共用）
│   ├── GatewayClass.yaml
│   ├── Gateway.yaml
│   └── EdgionGatewayConfig.yaml
├── http/                   # HTTP suite
│   ├── Service.yaml
│   ├── EndpointSlice.yaml
│   └── HTTPRoute.yaml
├── https/                  # HTTPS suite
├── grpc/                   # gRPC suite
├── tcp/                    # TCP suite
├── udp/                    # UDP suite
└── websocket/              # WebSocket suite
```

## 加载顺序

1. **base/** - 必须首先加载，包含 GatewayClass、Gateway 等基础资源
2. **各 suite 目录** - 按需加载测试所需的配置

## 使用方法

```bash
# 加载基础配置
./examples/test/scripts/utils/load_conf.sh base

# 加载特定 suite 配置
./examples/test/scripts/utils/load_conf.sh http

# 加载所有配置
./examples/test/scripts/utils/load_conf.sh all

# 列出可用 suite
./examples/test/scripts/utils/load_conf.sh --list
```

## 配置说明

### base/

- **GatewayClass.yaml**: 定义 Gateway 类型，关联 EdgionGatewayConfig
- **Gateway.yaml**: 定义监听器（HTTP:10080, HTTPS:10443, TCP:19000, UDP:19002）
- **EdgionGatewayConfig.yaml**: Edgion 特定配置（超时、Real IP、安全防护等）

### http/

- **Service.yaml**: HTTP 测试服务定义
- **EndpointSlice.yaml**: 后端服务发现（指向 127.0.0.1:30001）
- **HTTPRoute.yaml**: HTTP 路由规则（Host: test.example.com）

## 添加新 suite

1. 创建对应目录（如 `grpc/`）
2. 添加所需配置文件（Service、EndpointSlice、Route 等）
3. 在 `load_conf.sh` 中注册 suite
