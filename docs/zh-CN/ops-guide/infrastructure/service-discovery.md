# 服务发现集成

Edgion 支持在不修改核心代码的前提下，集成各种主流的外部服务发现注册中心（如 HashiCorp Consul、Nacos、Eureka 以及 Apache ZooKeeper）。这种集成是通过 **注册中心同步桥接器（Registry Sync Bridge）** 模式实现的。

通过将服务发现同步器与控制面和数据面剥离，Edgion 能够保持极高的轻量化和性能，且完全不受第三方客户端 SDK 生命周期的影响。

---

## 架构设计原理

无论是 Kubernetes 还是非 Kubernetes 环境，Edgion 都统一使用 Kubernetes 规范的资源来表示后端服务：`Service` 和 `EndpointSlice`。

外部服务发现集成依赖于一个**同步桥接器（Sync Bridge）**（可以是一个 Operator、Sidecar 守护进程或简单的定时脚本）。该桥接器负责订阅源注册中心，将服务实例（IP/端口）翻译为 Edgion 兼容的 `EndpointSlice` 资源，并将其写入 Edgion 的配置中心（ConfCenter）。

```
+-----------------------------------------------------------+
|                    外部服务注册中心                       |
|           (Consul / Nacos / Eureka / ZooKeeper)           |
+---------------------+-------------------------------------+
                      | (监听 / 轮询)
                      v
+---------------------+-------------------------------------+
|                  注册中心同步桥接器                       |
|   (例如 consul-k8s, consul-template 或自定义同步器)       |
+---------------------+-------------------------------------+
                      | (写入配置)
                      v
+---------------------+-------------------------------------+
|                  Edgion 配置中心                          |
|       (Kubernetes API Server / Etcd / 文件系统)           |
+---------------------+-------------------------------------+
                      | (动态监听)
                      v
+---------------------+-------------------------------------+
|                 Edgion 控制器 (Controller)                |
+---------------------+-------------------------------------+
                      | (gRPC 配置同步)
                      v
+---------------------+-------------------------------------+
|                 Edgion 网关 (Gateway)                     |
|                   (数据面负载均衡)                        |
+-----------------------------------------------------------+
```

---

## 端点就绪语义

Edgion 遵循 Kubernetes `discovery.k8s.io/v1` 对 `endpoints[].conditions.ready` 的约定：

| `conditions.ready` | Gateway 行为 |
|:---|:---|
| `true` | 端点接收流量。 |
| 省略（没有 `conditions`、`conditions` 为空或没有 `ready` 字段） | 视为就绪并接收流量，与 kube-proxy 一致。 |
| `false` | 端点不参与负载均衡。 |

因此，同步桥接器无需为健康实例写入 `conditions`。要将某个实例摘除，可以从 slice 中删除该端点，或设置 `ready: false`：

```yaml
apiVersion: discovery.k8s.io/v1
kind: EndpointSlice
metadata:
  name: my-service-slice
  namespace: default
  labels:
    kubernetes.io/service-name: my-service
addressType: IPv4
ports:
  - name: http
    port: 8080
endpoints:
  # 未设置 conditions：视为就绪
  - addresses:
      - "10.0.0.11"
  # 显式未就绪：不参与负载均衡
  - addresses:
      - "10.0.0.12"
    conditions:
      ready: false
```

注册中心实例状态的推荐映射：

| 注册中心 | 实例状态 | EndpointSlice 端点 |
|:---|:---|:---|
| Consul | 所有健康检查均为 `passing` 或 `warning`（Consul DNS 默认也包含 `warning`） | 就绪（省略 `conditions` 或设置 `ready: true`） |
| Consul | 任一检查为 `critical`，或处于维护模式 | `ready: false`，或不写入该端点 |
| Nacos | `healthy: true` 且 `enabled: true` | 就绪 |
| Nacos | `healthy: false` 或 `enabled: false` | `ready: false`，或不写入该端点 |
| Eureka | `UP` | 就绪 |
| Eureka | `DOWN`、`OUT_OF_SERVICE`、`STARTING` 或 `UNKNOWN` | `ready: false`，或不写入该端点 |

> `ready` 只是同步桥接器写入 slice 时的快照，两次同步之间没有任何组件会刷新它。为了及时摘除故障实例，建议为同步出来的 Service 绑定带有[主动健康检查](../../user-guide/http-route/backends/health-check.md)或[异常检测（熔断）](../../user-guide/http-route/backends/circuit-breaker.md)的 `EdgionBackendTrafficPolicy`。
>
> 目前 Gateway 只根据 `ready` 选择端点，不使用 `serving` 和 `terminating` 条件。

---

## 1. HashiCorp Consul 集成

### Kubernetes 环境 (consul-k8s Catalog Sync)

在 Kubernetes 环境下，您可以使用 HashiCorp 官方的 `consul-k8s` 工具中的 **Catalog Sync** 功能，直接将 Consul 服务同步到 Kubernetes 中。

1. **部署 Catalog Sync**：配置 Consul Helm Chart 启用 Catalog Sync：
   ```yaml
   syncCatalog:
     enabled: true
     toK8s: true # 将 Consul 服务同步到 K8s
   ```
2. **自动协调**：`consul-k8s` 会自动查询 Consul 服务目录，并在指定的 K8s 命名空间内创建对应的 `Service` 和 `Endpoints` 资源。
3. **EndpointSlice 自动生成**：Kubernetes 内置的 `endpointslice-controller` 会自动将生成的 `Endpoints` 转换为标准的 `EndpointSlice` 资源。
4. **Edgion 消费**：Edgion 控制器原生监听这些生成的 `EndpointSlice`，编译后推送给 `edgion-gateway` 进行动态负载均衡。

### 非 Kubernetes 环境 (consul-template)

在纯 VM 或物理机环境下（使用 `file_system` 或 `etcd` 配置中心），您可以使用 `consul-template` 动态写入配置文件。

1. **编写模板文件**：创建 `endpoint_slice.yaml.tpl` 模板：
   ```yaml
   apiVersion: discovery.k8s.io/v1
   kind: EndpointSlice
   metadata:
     name: my-service-slice
     namespace: default
   addressType: IPv4
   ports:
     - name: http
       port: 8080
   endpoints:
     {{range service "my-service"}}
     - addresses:
         - "{{.Address}}"
       conditions:
         ready: true
     {{end}}
   ```
2. **运行 consul-template**：以 systemd 服务的形式运行 `consul-template`，实时监听 Consul 服务变更并输出到 Edgion 的文件配置目录：
   ```bash
   consul-template \
     -template "/etc/edgion/templates/endpoint_slice.yaml.tpl:/etc/edgion/conf/EndpointSlice/default/my-service-slice.yaml"
   ```
3. **热重载**：当 Consul 中的服务实例发生增删时，`consul-template` 会瞬间重写 YAML 文件，Edgion 内部的文件监听器会立即感知并平滑热重载路由后端，期间不中断连接。

---

## 2. 阿里巴巴 Nacos 集成

### Kubernetes 环境 (nacos-controller / nacos-istio)

要将 Nacos 与 Kubernetes 整合，可以使用社区维护的 `nacos-controller` 或类似的同步器：

1. **部署**：在 K8s 集群中部署 Nacos 同步控制器。
2. **同步**：该控制器轮询 Nacos 实例接口，在 K8s 中自动创建对应的 `Service` 和 `Endpoints`。
3. **消费**：Edgion 自动通过 K8s API 监听对应的 `EndpointSlice` 变化。

### 非 Kubernetes 环境 (Nacos 自动同步脚本 / Daemon)

在虚拟机环境部署时，建议编写一个轻量级的同步脚本（如 Go/Python），定期或通过长轮询请求 Nacos HTTP API。

1. **请求 Nacos API**：请求 `/nacos/v1/ns/instance/list?serviceName=my-service` 接口获取实例列表。
2. **生成 YAML 资源**：解析返回的 JSON，提取所有就绪实例的 `ip` 和 `port`，格式化为 Edgion 的 `EndpointSlice` 规范 YAML。
3. **写入/调用 API**：将文件保存到 `/etc/edgion/conf/EndpointSlice/default/my-service.yaml`，或调用 Edgion 控制器的 Admin API：
   ```bash
   curl -X PUT \
     -H "Authorization: Bearer <your-admin-token>" \
     -H "Content-Type: application/yaml" \
     --data-binary @my-service-slice.yaml \
     http://127.0.0.1:12101/api/v1/namespaced/EndpointSlice/default/my-service-slice
   ```

---

## 3. Spring Cloud Eureka 集成

Eureka 是基于 REST 的服务注册中心。由于 Eureka 没有 sidecar 代理，通常通过定时轮询桥接器来实现同步。

### Kubernetes 环境 (Spring Cloud Kubernetes)

当您在 Kubernetes 中运行 Spring Cloud 应用时：
1. 部署 **Spring Cloud Kubernetes Discovery Server**，将 Eureka 注册表同步为 Kubernetes 原生 Service。
2. Edgion 即可像普通 K8s Service 一样对其进行解析和负载均衡。

### 非 Kubernetes 环境 (Eureka 桥接 Daemon)

可以在 Edgion 控制器旁部署一个轻量级的同步组件：
1. **轮询 Eureka**：调用 Eureka REST 接口 `GET /eureka/apps/MY-SERVICE`。
2. **格式转换**：将返回的 XML/JSON 数据中包含的实例 IP 和 Port 组装成 Edgion 的 `EndpointSlice` 资源对象。
3. **推送到 Edgion**：直接写入本地配置文件，或者通过 Edgion 控制器的 Admin API 更新端点。

---

## 4. Apache ZooKeeper 集成 (Dubbo / Java 微服务)

ZooKeeper 以树状 ZNode 节点结构保存 Dubbo 等微服务的服务提供者地址（例如 `/dubbo/com.foo.BarService/providers`）。

### ZooKeeper 监听同步器模式

由于 ZooKeeper 不是 REST 接口而是树状结构，建议使用一个常驻的 Watcher 同步进程：

1. **监听 ZooKeeper 路径**：同步进程连接 ZK 并对 `/dubbo/{service_name}/providers` 节点注册 Watcher 监听。
2. **解析节点事件**：当提供者节点上线或下线时，ZK 触发 Watch 回调。同步进程读取最新的子节点列表，并对 URL 编码的提供者字符串进行解析，提取 IP 与端口。
3. **分发到 Edgion**：将最新的实例列表转换成 `EndpointSlice` 规范，通过 Edgion 控制器 Admin API 完成对 Gateway 后端的无损更新。

---

## 常见集成策略总结

| 部署环境 | 服务注册中心 | 桥接/同步工具 | 输出资源 |
|:---|:---|:---|:---|
| **Kubernetes** | Consul | `consul-k8s` catalog sync | 原生 `Service` & `EndpointSlice` |
| **Kubernetes** | Nacos / Eureka | 同步 Operator / 控制器 | 原生 `Service` & `EndpointSlice` |
| **VM / 裸金属** | Consul | `consul-template` | 文件 / Etcd 中的 `EndpointSlice` YAML |
| **VM / 裸金属** | Nacos / Eureka / ZK | 自定义同步守护进程 / 脚本 | 文件 / Etcd 中的 `EndpointSlice` YAML (通过 Admin API 写入) |

通过引入这些同步模式，Edgion 不仅维持了高度精简的代码边界，还完美兼容了各种传统的外部服务发现后端，且网关本身没有任何额外的性能损耗。
