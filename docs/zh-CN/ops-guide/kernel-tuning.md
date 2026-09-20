# Linux 内核与资源调优

## 概述

这套可选基线用于减少常见的文件描述符（FD）和监听队列瓶颈，适合作为专用 Linux 网关的起点，但不代表容量保证，也不会在启动时自动执行。生产部署前，应结合实际 CPU 配额、内存限制、协议比例、连接时长、TLS 和上游行为进行测试。

**基线不设置 `net.ipv4.tcp_tw_reuse`，也不会强制设置为 `2`。** 该参数保留内核、发行版及运维策略决定的现有值。

## 基线配置

| 参数 | 类型 | 模板值 | 说明 |
|---|---|---|---|
| `net.core.somaxconn` | 整数 | `65535` | 对齐当前 Edgion Pingora 的监听 backlog，不是活跃连接数上限。 |
| `LimitNOFILE` | 整数 | `1048576` | systemd 的 FD 软、硬上限；不会预分配一百万个 socket。 |
| PAM `nofile` | 整数 | `1048576` | PAM 会话中专用 `edgion` 用户的软、硬上限，作为替代方案。 |
| `net.ipv4.tcp_tw_reuse` | 整数 | 不修改 | 不覆盖默认策略，即使当前值为 `0` 或 `1`。 |

这些是**模板值**，不是 Linux 或 Edgion 的运行时默认值。模板位于源码仓库的 `cicd/deploy/linux-tuning/`，Edgion 不会自行安装。已有 backlog 如果更高，应先确认其用途，不要直接降低。扩大队列会增加排队工作量和内核内存占用，不能解决 accept 持续处理不及时的问题。

## 手动安装示例

### 安装前检查

在目标 Linux 环境执行以下只读命令：

```bash
sysctl net.core.somaxconn net.ipv4.tcp_tw_reuse
sysctl fs.nr_open fs.file-max
ulimit -Sn
ulimit -Hn
systemctl cat edgion-gateway.service
systemctl show edgion-gateway.service -p LimitNOFILE -p LimitNOFILESoft -p TasksMax
```

服务命令假设已存在名为 `edgion-gateway.service` 的**运维自备 unit**，请替换为实际名称。记录原始运行时值，并将已有目标文件备份到 `sysctl.d`、`limits.d` 和服务 drop-in 目录之外。检查其他覆盖配置，文件排序可能影响最终值。要求 `fs.nr_open >= 1048576`；否则应降低 FD 上限，或单独评审主机级限制调整。同时检查所有进程的 FD 总预算是否符合 `fs.file-max`，进程额度并不等于主机资源预留。

### 使用 systemd 的裸机或虚拟机

审阅模板后，从仓库根目录执行。`test` 检查会拒绝覆盖已有文件；遇到已有文件时请人工合并。

```bash
sudo install -d -m 0755 /etc/sysctl.d
sudo test ! -e /etc/sysctl.d/99-edgion.conf && \
  sudo install -m 0644 cicd/deploy/linux-tuning/sysctl.d/99-edgion.conf /etc/sysctl.d/99-edgion.conf
sudo sysctl -p /etc/sysctl.d/99-edgion.conf

sudo install -d -m 0755 /etc/systemd/system/edgion-gateway.service.d
sudo test ! -e /etc/systemd/system/edgion-gateway.service.d/90-resource-limits.conf && \
  sudo install -m 0644 cicd/deploy/linux-tuning/systemd/edgion-gateway.service.d/90-resource-limits.conf /etc/systemd/system/edgion-gateway.service.d/90-resource-limits.conf
sudo systemctl daemon-reload
# Schedule/drain traffic before restarting the service.
sudo systemctl restart edgion-gateway.service
```

逐步执行，失败就停止。sysctl 命令立即作用于当前网络命名空间，FD 配置则需要重启服务。本指南不提供完整服务 unit，也不预设 `LimitNPROC`。遇到线程或进程创建失败，应检查 `TasksMax`、cgroup `pids.max` 及实际任务数；`LimitNPROC` 按真实 UID 计数，不是连接数限制。

### PAM 替代方案

对于通过 PAM 会话启动的非 systemd 服务，先调整 `cicd/deploy/linux-tuning/limits.d/99-edgion.conf` 中的专用用户名，确认登录路径加载 `pam_limits`，备份已有目标文件后，手动安装到 `/etc/security/limits.d/99-edgion.conf`。建立**新会话**，检查 `ulimit -Sn` 和 `ulimit -Hn`，再从该会话重启网关。此方式不会配置 systemd 服务、已运行进程或容器运行时的限制。

### 容器与 Kubernetes

通过容器运行时或节点服务配置 FD 上限，并检查真实网关进程。非特权 Shell 无法提高继承的硬上限。网络 sysctl 应检查 **Pod 的网络命名空间**，不能只查看节点；节点设置不一定决定已存在 Pod 的有效值。Kubernetes 只允许符合准入规则的命名空间级 sysctl；unsafe sysctl 需要节点及集群策略显式允许，`hostNetwork` Pod 禁止配置 `net.*` sysctl。

本基线不部署特权调优 DaemonSet。请与集群管理员协调节点策略，参见 [Kubernetes sysctl 指南](https://kubernetes.io/docs/tasks/administer-cluster/sysctl-cluster/)。

## 按需调优，不作为默认值

| 参数 | 类型 | 基线行为 | 排查条件 |
|---|---|---|---|
| `tcp_tw_reuse` | 整数 | 保留 | 优化连接复用后，仍确认存在出站临时端口压力。 |
| `ip_local_port_range`、`ip_local_reserved_ports` | 范围 / 列表 | 保留 | 端口分配失败；扩展前核对服务端口及保留范围。 |
| `tcp_max_syn_backlog` | 整数 | 保留 | SYN 队列压力；将突发流量或攻击与已完成握手的 accept 队列分开排查。 |
| `netdev_max_backlog` | 整数 | 保留 | 检查 CPU、RSS、IRQ 分布及包处理后仍存在 NIC/softnet 丢包。 |
| `tcp_rmem`、`tcp_wmem`、`rmem_max`、`wmem_max` | 字节 / 三元组 | 保留 | 已测得带宽时延需求；保留自动调节并核算 socket 总内存。 |
| `tcp_fin_timeout` | 秒 | 保留 | 孤儿连接的 `FIN_WAIT_2`；**不是** `TIME_WAIT` 时长控制。 |
| `tcp_keepalive_time`、`tcp_keepalive_intvl`、`tcp_keepalive_probes` | 秒 / 次数 | 保留 | 失活对端探测需求；仅对启用了 TCP keepalive 的 socket 有关，并可能被 socket 级配置覆盖。 |

当前 [Linux TCP 文档](https://docs.kernel.org/networking/ip-sysctl.html)定义 `tcp_tw_reuse`：`0` 关闭、`1` 全局、`2` 仅 loopback，文档默认值为 `2`。旧版或厂商内核可能不同，应核对实际内核文档和有效值。设置 `1` 会扩大到非 loopback 连接，并非所有代理都需要。只有确认出站连接频繁新建、关闭，并验证真实对端、时间戳、NAT 和故障行为后才考虑启用；不要盲目降低 `tcp_tw_reuse_delay`。记录原值用于回滚。仅有大量 `TIME_WAIT` 不代表发生故障，已关闭的 `TIME_WAIT` socket 不占用进程的存活 FD。

不要照搬包含 `tcp_tw_recycle` 的历史配置。本基线不关闭 TCP 安全机制，也不统一扩大端口范围和缓冲区。

## 运行时与分配器配合

以下是已有的进程本地 YAML 配置，需要重启，不是 CRD 字段：

```yaml
server:
  threads: 0
  work_stealing: true
  upstream_keepalive_pool_size: 128
```

`threads: 0` 或省略时使用可用并行度，获取失败则使用一个线程。应检查实际 CPU 配额和节流情况，受限容器不要直接照搬宿主机核数。Work stealing 默认开启。连接池配置默认每 worker `128`，当前 fork 按 `pool_size * threads` 计算 connector 的池容量，但这**不是整个网关全部活跃和空闲上游连接的统一上限**。还需考虑多个连接池、connector 和 HTTP/2 多路复用。扩大池可减少握手和端口周转，也会保留更多 FD 和内存。

默认分配器构建使用带前缀的 tikv-jemalloc，其环境变量是 `_RJEM_MALLOC_CONF`，不能假定无前缀的 `MALLOC_CONF` 有效。以下仅作为有测量依据的预发布实验，在进程启动前设置：

```bash
export _RJEM_MALLOC_CONF='background_thread:true,dirty_decay_ms:30000,muzzy_decay_ms:30000'
```

基线不会安装该设置。较长 decay 可能保留更多驻留内存，后台回收也会消耗线程和 CPU。与未修改的对照组比较 RSS、cgroup 内存、延迟和 CPU。该变量不调节 mimalloc 或系统分配器构建。堆分析需要编译时启用 jemalloc profiling 支持，普通构建没有开启此功能。请使用单独的诊断构建，计入分析开销，并将堆转储作为敏感数据保护。参见 [jemalloc 手册](https://jemalloc.net/jemalloc.3.html)。

## 容量、验证与告警

应保证“下游 FD + 活跃上游 FD + 空闲上游 FD + 其他 FD + 余量”低于进程软上限。简单的一对一代理连接对大致需要两个 socket FD，但这不是通用容量公式；HTTP/2、流式请求、插件和并行上游调用都会改变比例。允许一百万个 FD **不等于**可以安全承载一百万条连接。

```bash
edgion_pid=$(systemctl show edgion-gateway.service -p MainPID --value)
# Confirm a nonzero PID belonging to the gateway before proceeding.
cat /proc/"$edgion_pid"/limits
ls -1 /proc/"$edgion_pid"/fd | wc -l
ss -lnt
ss -s
ss -tan state time-wait | wc -l
nstat -az TcpExtListenOverflows TcpExtListenDrops
sysctl net.core.somaxconn net.ipv4.tcp_tw_reuse
```

使用适当权限，并进入网关所在的网络命名空间。精简容器可能需要共享该命名空间的诊断工具容器。`ss`/`nstat` 是命名空间级观测，不是网关专属指标；应对比测试区间内的增量。`ss` 行数包含表头。对于监听 TCP socket，`ss -lnt` 展示排队连接数和 backlog 上限，重启后应确认实际 listener。验证 `/proc/<pid>/limits`，不要只看交互式 Shell 的 `ulimit`。

压测应覆盖稳定流量、连接突发与短连接周转、慢客户端和慢上游、长流及滚动重启。记录吞吐、p95/p99 延迟、错误、CPU 节流、RSS/cgroup 内存及 OOM、FD 利用率、队列丢弃、端口分配失败，以及适用场景下的 conntrack 占用。按真实负载测量单连接增量内存，不要假定固定字节成本。

保留 CPU **以及**内存、FD、队列、端口和 conntrack 告警。FD 70%/85% 配合持续时间窗口可作为 warning/critical 的初始参考，不是通用默认值。OS 信号由主机或进程监控采集，本指南不新增 Edgion 内置指标。提高资源额度不是过载保护，仍需按部署需要设置准入、限流和负载丢弃策略。

## 回滚

恢复备份文件，或仅将本次新安装的 Edgion 文件移出相应配置扫描目录。删除 sysctl 文件不会撤销已生效值，应使用 `sudo sysctl -w net.core.somaxconn=VALUE` 恢复记录的原值（替换 `VALUE`），立即检查并在下次重启后再次检查。不修改 `tcp_tw_reuse`；如果在单独实验中调整过，则恢复其记录值。

恢复或移除 systemd drop-in 后执行 `daemon-reload`，安排服务重启，再检查 `/proc/<pid>/limits`。PAM 回滚需要新会话及进程重启。移除实验性的分配器环境变量后也需要重启进程。降低资源限制前先排空流量。

## 相关文档

- [Gateway 进程配置](./configuration/gateway-config.md)
- [监控指标](./observability/metrics.md)
- [Linux 网络 sysctl](https://docs.kernel.org/admin-guide/sysctl/net.html)
- [systemd 资源限制](https://www.freedesktop.org/software/systemd/man/latest/systemd.exec.html)
- [PAM limits](https://man7.org/linux/man-pages/man8/pam_limits.8.html)
- [Envoy 资源限制](https://www.envoyproxy.io/docs/envoy/latest/faq/configuration/resource_limits)：连接限制需要与 FD 余量配合。
- [Envoy 边缘部署实践](https://www.envoyproxy.io/docs/envoy/latest/configuration/best_practices/edge)：内存与过载保护仍然必要。
- [Traefik HTTP transport](https://doc.traefik.io/traefik/reference/routing-configuration/http/load-balancing/serverstransport/) 和 [TCP 并发连接限制](https://doc.traefik.io/traefik/reference/routing-configuration/tcp/middlewares/inflightconn/)：连接复用与显式并发限制是不同的调优手段。

这些 Envoy/Traefik 资料用于参考运维思路，并不证明它们将 `tcp_tw_reuse=1` 推荐为现代通用默认值。
