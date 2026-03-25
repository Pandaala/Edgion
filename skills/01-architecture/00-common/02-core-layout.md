---
name: core-layout
description: Core 模块分层规范：顶级分组、各 bin 布局、放置规则、anti-patterns。
---

# Core 分层规范

> 本文件是新增代码时的放置规则参考。

## 顶级分组

`src/core/` 只有四个顶级组：

```
src/core/
├── controller/   # edgion-controller 归属逻辑
├── gateway/      # edgion-gateway 归属逻辑
├── ctl/          # edgion-ctl 归属逻辑
└── common/       # 跨 bin 共享代码
```

**放置原则**：
- 只服务控制面 → `controller/`
- 只服务数据面 → `gateway/`
- 只服务 CLI → `ctl/`
- 至少两个 bin 依赖且无隐式运行时耦合 → `common/`

## Gateway 布局

按子系统组织，而非按技术原语：

```
src/core/gateway/
├── api/          # Admin/测试 API
├── backends/     # 后端发现 / 健康检查 / BackendTLSPolicy
│   ├── discovery/    # Endpoint / EndpointSlice / Service 发现
│   ├── health/       # 主动健康检查
│   ├── policy/       # 后端 TLS 策略
│   ├── preload/      # LB 预加载
│   └── validation/   # 端点验证
├── cache/        # LRU 缓存
├── cli/          # CLI 入口 + Pingora 启动
├── config/       # GatewayClass / EdgionGatewayConfig 处理器和存储
├── conf_sync/    # gRPC 配置客户端 + ClientCache
│   ├── conf_client/  # gRPC 客户端实现
│   └── cache_client/ # 本地缓存 + 事件分发
├── lb/           # 负载均衡策略实现
│   ├── backend_selector/  # 加权轮询选择器
│   ├── ewma/              # EWMA 算法
│   ├── leastconn/         # 最少连接
│   ├── lb_policy/         # LB 策略定义
│   ├── runtime_state/     # 每后端运行时状态
│   └── selection/         # 算法工厂
├── link_sys/     # 外部系统提供程序 + 运行时存储
│   ├── providers/    # Redis / Etcd / ES / Webhook / LocalFile
│   └── runtime/      # LinkSysStore + DataSender
├── observe/      # AccessLog / Metrics / SSL/TCP/TLS/UDP 日志
├── plugins/      # 插件系统
│   ├── http/         # 28 个 HTTP 插件实现
│   ├── stream/       # Stream 插件（IP 限制 + TLS 路由）
│   └── runtime/      # PluginRuntime + 条件 + Gateway API 过滤器
├── routes/       # 路由处理
│   ├── http/         # HTTPRoute 匹配 + proxy_http
│   ├── grpc/         # GRPCRoute 匹配 + gRPC 集成
│   ├── tcp/          # TCPRoute 运行时
│   ├── tls/          # TLSRoute 运行时
│   └── udp/          # UDPRoute 运行时
├── runtime/      # Pingora 运行时核心
│   ├── server/       # Listener 构建器 + 错误响应
│   ├── matching/     # Gateway/Route/TLS 匹配
│   └── store/        # Gateway + Route 配置存储
├── services/     # Gateway 侧服务（ACME challenge）
└── tls/          # TLS 管理
    ├── boringssl/    # BoringSSL 后端（feature）
    ├── openssl/      # OpenSSL 后端（feature）
    ├── runtime/      # 下游/上游 TLS 回调
    ├── store/        # 证书存储 + SNI 匹配
    └── validation/   # 证书验证
```

**二级规则**：
- `runtime/` — 仅限 Pingora 运行时核心
- `routes/` — 仅限路由管理器、匹配器、协议服务
- `plugins/` — 仅限插件实现和执行框架
- `backends/` — 仅限上游发现、健康过滤、后端策略
- `tls/` — 仅限下游 TLS 证书处理和验证
- `link_sys/` — 仅限 LinkSys 声明的外部系统
- `config/` — 仅限影响网关运行时的配置资源，不放 gRPC 同步机制

## Controller 布局

```
src/core/controller/
├── api/          # Admin API 端点
│   ├── cluster_handlers/      # 集群级资源 CRUD
│   ├── namespaced_handlers/   # 命名空间级资源 CRUD
│   ├── configserver_handlers/ # ConfigServer 端点（for ctl）
│   └── common/                # 共享处理逻辑
├── cli/          # CLI 入口和启动
├── conf_mgr/     # 配置管理器（核心）
│   ├── conf_center/           # 配置源抽象
│   │   ├── file_system/       #   FileSystemCenter 实现
│   │   └── kubernetes/        #   KubernetesCenter 实现
│   ├── sync_runtime/          # 同步运行时
│   │   ├── workqueue/         #   工作队列
│   │   ├── resource_processor/#   资源处理管道
│   │   │   ├── handlers/      #     23 种资源处理器
│   │   │   ├── ref_grant/     #     跨命名空间引用验证
│   │   │   ├── secret_utils/  #     Secret 依赖跟踪
│   │   │   └── configmap_utils/#    ConfigMap 工具
│   │   ├── shutdown/          #   优雅关闭
│   │   └── metrics/           #   Prometheus 指标
│   ├── processor_registry/    # 全局处理器注册表
│   └── schema_validator/      # CRD Schema 验证
├── conf_sync/    # 配置同步（gRPC 服务端）
│   ├── conf_server/           # ConfigSyncServer + gRPC 实现
│   └── cache_server/          # ServerCache + EventStore
├── observe/      # 日志 facade
└── services/     # Controller 侧服务
    └── acme/                  # ACME 证书自动化
```

**放置规则**：
- K8s watch/list、parse/preparse、引用管理、状态回写 → `conf_mgr/`
- 面向 Gateway 的配置分发 → `conf_sync/`

## Common 布局

刻意保持小规模：

```
src/core/common/
├── conf_sync/    # 共享 proto / traits / 同步类型
│   ├── proto/        # protobuf 定义
│   ├── traits/       # CacheEventDispatch + ConfHandler
│   └── types/        # 同步相关类型
├── config/       # 共享启动配置
├── matcher/      # 共享匹配算法
│   ├── host_match/   # 域名匹配（Hash + Radix）
│   ├── ip_radix_tree/ # IP Radix 树
│   └── radix_tree/   # 通用 Radix 树
└── utils/        # 可复用工具
    ├── net/              # IP 验证、解析
    ├── duration/         # 时间解析
    ├── real_ip_extractor/# 真实 IP 提取
    ├── metadata_filter/  # 元数据清理
    └── proxy_protocol/   # PROXY Protocol
```

不要仅为了避免跨模块导入就把代码移到 `common/`。保持业务归属明确。

## Types 布局

```
src/types/
├── resource/         # 资源系统基础设施
│   ├── kind/         #   ResourceKind 枚举
│   ├── defs/         #   define_resources! 宏声明
│   ├── macros/       #   宏定义
│   ├── meta/         #   ResourceMeta trait
│   └── registry/     #   资源类型注册表
├── resources/        # 各类资源结构体（对应 CRD）
│   ├── gateway/      #   Gateway
│   ├── http_route/   #   HTTPRoute
│   ├── edgion_plugins/#  EdgionPlugins
│   └── ...           #   其余 17 种资源
├── common/           # KeyGet/KeySet 统一访问器
├── constants/        # 集中式常量
│   ├── app/          #   应用标识符
│   ├── labels/       #   K8s 标签
│   ├── annotations/  #   K8s 注解
│   ├── headers/      #   HTTP 头部
│   └── secret_keys/  #   Secret 数据键
├── ctx.rs            # EdgionHttpContext
├── filters.rs        # PluginRunningResult
├── schema.rs         # JSON Schema
├── err.rs            # 错误类型
├── work_dir.rs       # 工作目录
└── observe.rs        # 可观测配置
```

## Anti-Patterns

避免重新引入以下模式：

| 反面模式 | 正确做法 |
|---------|---------|
| 顶层 `src/core/api`、`src/core/cli`、`src/core/conf_sync` | 放在对应 bin 组下 |
| 扁平 gateway 桶如 `gateway/http_routes`、`gateway/health_check` | 按子系统组织 |
| 新的兼容性 shim 隐藏真实归属 | 直接放在正确的 bin 组 |
| 在 `types/` 中放执行逻辑 | types 只放数据定义 |
| 在 gateway 中直接调用 kube-rs API | 通过 ConfigClient 从 Controller 同步 |
| 插件直接访问数据库/外部系统 | 通过 LinkSysStore 中的客户端 |

如果新子系统需要归属，优先在正确的 bin 组下创建清晰的目录，而不是添加另一个跨领域的顶层桶。
