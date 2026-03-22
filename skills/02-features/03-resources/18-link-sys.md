---
name: link-sys-detail
description: LinkSys CRD 完整 Schema：Redis/Etcd/Elasticsearch/Webhook/Kafka 连接器配置。
---

# LinkSys 资源

> API: `edgion.io/v1` | Scope: Namespaced

LinkSys 是统一的外部系统连接器资源，通过 `config.type` 区分不同后端。

## 基础 Schema

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: my-redis
  namespace: default
spec:
  config:
    type: Redis                        # Redis | Etcd | Elasticsearch | Webhook | Kafka
    config:
      # 类型特定配置
      addresses:
        - "redis://127.0.0.1:6379"
```

## 系统类型

### Redis

```yaml
spec:
  config:
    type: Redis
    config:
      addresses:                       # Redis 地址列表
        - "redis://127.0.0.1:6379"
      # 其他 Redis 连接参数
```

**用途**：分布式限流（RateLimitRedis 插件）、分布式会话等。

### Etcd

```yaml
spec:
  config:
    type: Etcd
    config:
      endpoints:                       # Etcd 端点列表
        - "http://127.0.0.1:2379"
```

**用途**：配置存储、服务发现。

### Elasticsearch

```yaml
spec:
  config:
    type: Elasticsearch
    config:
      addresses:                       # ES 节点地址
        - "http://127.0.0.1:9200"
```

**用途**：日志存储（Access Log 输出到 ES）。

### Webhook

```yaml
spec:
  config:
    type: Webhook
    config:
      url: "https://webhook.example.com/notify"
      # Webhook 管理器配置
```

**用途**：事件通知、外部系统回调。

### Kafka

```yaml
spec:
  config:
    type: Kafka
    config:
      brokers:                         # Kafka broker 列表
        - "kafka:9092"
```

**用途**：消息队列集成（占位，功能开发中）。

## Status Schema

```yaml
status:
  conditions:
    - type: Accepted                   # 配置已验证
      status: "True"
    - type: Ready                      # 连接已建立
      status: "True"
```

## 运行时行为

- Controller 侧：验证配置 → 管理 status
- Gateway 侧：`LinkSysStore` 管理各类型运行时客户端
- 配置变更时 `ConfHandler` 通过 `partial_update` 重建受影响的客户端
