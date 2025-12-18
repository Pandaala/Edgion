# Edgion 配置示例

网关配置示例文件，用于快速测试。

## 文件列表

- `EdgionGatewayConfig__example-gateway.yaml` - 网关全局配置

## 使用方法

```bash
# 启动 controller（加载配置）
cargo run --bin edgion-controller -- \
  --gateway-class example-gateway \
  --loader-dir examples/conf

# 启动 gateway
cargo run --bin edgion-gateway -- \
  --gateway-class example-gateway
```

## 配合测试

```bash
# 启动测试服务器
cargo run --example test_server

# 运行测试
cargo run --example test_client -- all
```
