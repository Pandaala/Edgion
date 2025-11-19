# Edgion


todolist


plugins
- 内部增加一个配置，is_record_time_cost()函数，如果有的话，就在插件前后增加耗时计算




# edgion-op
./edgion-op --config edgion/config/edgion-op.toml

## 使用默认配置
./edgion-op

## 命令行参数覆盖配置文件
./edgion-op -c edgion/config/edgion-op.toml --log-level debug --grpc-listen "0.0.0.0:50052"

## 查看帮助
./edgion-op --help


## 基本启动（使用默认配置）
cargo run --bin edgion-op -- --gateway-class my-gateway-class

## 完整参数示例
cargo run --bin edgion-op -- \
--gateway-class public-gateway \
--grpc-listen 127.0.0.1:50061 \
--admin-listen 127.0.0.1:8080 \
--loader-type localpath \
--loader-dir config/examples \
--log-level debug

必需参数：
--gateway-class: Gateway class 名称
常用可选参数：
--grpc-listen: gRPC 监听地址（默认：127.0.0.1:50061）
--admin-listen: Admin HTTP 监听地址
--loader-type: 配置加载器类型（filesystem 或 etcd）
--loader-dir: 配置文件目录（filesystem loader）
--etcd-endpoint: etcd 端点（etcd loader）
--log-level: 日志级别（trace, debug, info, warn, error）

# edgion-gw
# 基本启动
cargo run --bin edgion-gw -- \
--gateway-class my-gateway-class \
--server-addr http://127.0.0.1:50061

# 完整参数示例
cargo run --bin edgion-gw -- \
--gateway-class my-gateway-class \
--server-addr http://127.0.0.1:50061 \
--admin-listen 127.0.0.1:8081