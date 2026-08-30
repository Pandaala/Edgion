# Task: 数据库与中间件七层数据代理 (Redis/MySQL/Kafka)

**优先级**: P3 (未来研究 / 商业特性储备)
**状态**: TODO
**相关模块**: `edgion-gateway/src/link_sys/`, `edgion-gateway/src/plugins/stream/`

---

## 1. 任务背景

在大型企业和安全敏感（如金融）场景下，对内部数据库（Redis、MySQL）和中间件（Kafka）的访问控制与审计是安全合规的刚需。
传统的通用四层 TCP 转发无法感知具体的数据库指令或 SQL 语句。通过在 Edgion 中引入针对特定二进制协议的 **L7 数据代理（Data Proxy）**能力，可以在不侵入业务应用的前提下，在网络边界实现免侵入的数据库 WAF、SQL 审计和 Topic 级中间件鉴权。

---

## 2. 技术演进规划

由于此类任务开发涉及大量的二进制网络协议编解码，我们将采用分阶段、按需落地的演进策略，并充分利用 Rust 的内存安全与高性能协议解析库。

### 阶段一：设计通用的 L4/L7 流式过滤器接口 (Stream Filter SDK)
*   **目标**：在 `edgion-gateway` 的 TCP/Stream 转发链路中，提炼出可用于拦截、修改和分析 TCP 字节流的 Filter 接口。
*   **任务**：
    1.  扩展 `StreamPlugin` 接口，支持对底层 TCP Socket 读写缓冲区的访问。
    2.  设计基于 Tokio Codec 的帧解码与状态机拦截模型，确保解析错误不会导致代理崩溃（Panic 防御）。

### 阶段二：Redis 代理与命令级治理 (第一里程碑)
*   **目标**：实现对 Redis RESP (Redis Serialization Protocol) 协议的深度解析与路由。
*   **任务**：
    1.  引入 Rust 的 RESP 协议编解码器（如 `redis-protocol`）。
    2.  解析 Redis 指令（如 `GET`/`SET`），实现基于 Key 的一致性哈希分流（配合 Maglev 算法支持 Redis Cluster 槽位路由）。
    3.  收集并导出命令级统计指标（如特定 Key 格式的请求频率、慢查询监控）。

### 阶段三：MySQL/PostgreSQL 审计与数据库安全网关
*   **目标**：解析 SQL 请求包，实现查询级审计与安全防护。
*   **任务**：
    1.  解析 MySQL 客户端-服务端协议报文。
    2.  提取 SQL 查询文本，对接轻量级正则或 AST 解析器，拦截典型 SQL 注入攻击。
    3.  输出结构化的数据库审计日志（记录：谁、在什么时间、对哪个实例、执行了什么 SQL 语句）。

### 阶段四：Kafka 细粒度权限控制 (ACLs)
*   **目标**：解析 Kafka API 报文，实现 Topic 级写入与消费控制。
*   **任务**：
    1.  解析 Kafka 传输协议帧，提取 API Key (如 Produce, Fetch) 和 Topic 名称。
    2.  结合 Pod 的 SPIFFE 身份，校验客户端是否拥有该 Topic 的读写权限，在网关边缘进行流量阻断。

---

## 3. 推荐引入的 Rust 开源技术栈

*   **协议解析**：`redis-protocol` (RESP 解析)、`mysql_common` (MySQL 协议报文解析)。
*   **流式处理**：`tokio-util::codec` (TCP 流到协议帧的双向转换)。
*   **数据脱敏/校验**：基于 Edgion DSL 扩展对流式二进制字段的匹配函数。
