# Edgion 文档目录树

> 自动维护的文档完整路径索引。修改文档结构后请同步更新。
> AI 可直接读取此文件定位任何文档。

## 顶层结构

```
docs/
├── README.md                       # 项目级 README（语言入口）
├── DIRECTORY.md                    # 本文件 — 完整目录树
├── refactoring/                    # 内部重构设计文档
├── review/                         # 代码审查记录
├── en/                             # English documentation
├── zh-CN/                          # 中文文档
└── ja/                             # 日本語ドキュメント（準備中）
```

## en/ — English

```
en/
├── README.md
├── getting-started/
│   ├── README.md
│   ├── installation-and-deployment.md
│   ├── first-gateway.md
│   └── core-concepts.md
├── user-guide/
│   ├── README.md
│   ├── http-route/
│   │   ├── README.md
│   │   ├── overview.md
│   │   ├── lb-algorithms.md
│   │   ├── matches/
│   │   │   ├── README.md
│   │   │   ├── path.md
│   │   │   ├── headers.md
│   │   │   ├── query-params.md
│   │   │   └── method.md
│   │   ├── filters/
│   │   │   ├── README.md
│   │   │   ├── overview.md
│   │   │   ├── plugin-composition.md
│   │   │   ├── gateway-api/
│   │   │   │   ├── request-header-modifier.md
│   │   │   │   ├── response-header-modifier.md
│   │   │   │   ├── request-mirror.md
│   │   │   │   ├── request-redirect.md
│   │   │   │   └── url-rewrite.md
│   │   │   └── edgion-plugins/
│   │   │       ├── README.md
│   │   │       ├── all-endpoint-status.md
│   │   │       ├── bandwidth-limit.md
│   │   │       ├── basic-auth.md
│   │   │       ├── cors.md
│   │   │       ├── csrf.md
│   │   │       ├── ctx-setter.md
│   │   │       ├── debug-access-log.md
│   │   │       ├── direct-endpoint.md
│   │   │       ├── dsl.md
│   │   │       ├── dynamic-upstream.md
│   │   │       ├── forward-auth.md
│   │   │       ├── header-cert-auth.md
│   │   │       ├── hmac-auth.md
│   │   │       ├── ip-restriction.md
│   │   │       ├── jwe-decrypt.md
│   │   │       ├── jwt-auth.md
│   │   │       ├── key-auth.md
│   │   │       ├── ldap-auth.md
│   │   │       ├── mock.md
│   │   │       ├── openid-connect.md
│   │   │       ├── proxy-rewrite.md
│   │   │       ├── rate-limit.md
│   │   │       ├── rate-limit-redis.md
│   │   │       ├── real-ip.md
│   │   │       ├── region-route.md
│   │   │       ├── request-mirror.md
│   │   │       ├── request-restriction.md
│   │   │       └── response-rewrite.md
│   │   ├── backends/
│   │   │   ├── README.md
│   │   │   ├── service-ref.md
│   │   │   ├── weight.md
│   │   │   ├── backend-tls.md
│   │   │   ├── health-check.md
│   │   │   └── circuit-breaker.md   # zh-CN 已有，en 待翻译
│   │   └── resilience/
│   │       ├── README.md
│   │       ├── timeouts.md
│   │       ├── retry.md
│   │       └── session-persistence.md
│   ├── grpc-route/
│   │   ├── README.md
│   │   ├── overview.md
│   │   ├── matches/overview.md
│   │   ├── filters/overview.md
│   │   └── backends/overview.md
│   ├── tcp-route/
│   │   ├── README.md
│   │   ├── overview.md
│   │   ├── stream-plugins.md
│   │   └── backends/overview.md
│   ├── udp-route/
│   │   ├── README.md
│   │   ├── overview.md
│   │   └── backends/overview.md
│   └── edgion-plugins/
│       ├── README.md
│       ├── real-ip.md
│       ├── rate-limit.md
│       ├── region-route.md
│       ├── openid-connect.md
│       ├── ldap-auth.md
│       ├── jwe-decrypt.md
│       ├── forward-auth.md
│       └── ctx-setter.md
├── ops-guide/
│   ├── README.md
│   ├── edgion-cli.md
│   ├── gateway/
│   │   ├── README.md
│   │   ├── overview.md
│   │   ├── gateway-class.md
│   │   ├── http-to-https-redirect.md
│   │   ├── preflight-policy.md
│   │   ├── listeners/
│   │   │   ├── README.md
│   │   │   ├── http.md
│   │   │   ├── https.md
│   │   │   ├── tcp.md
│   │   │   └── grpc.md
│   │   └── tls/
│   │       ├── README.md
│   │       ├── tls-termination.md
│   │       ├── edgion-tls.md
│   │       └── acme.md
│   ├── configuration/
│   │   ├── README.md
│   │   ├── controller-config.md
│   │   ├── gateway-config.md
│   │   └── edgion-gateway-config.md
│   ├── infrastructure/
│   │   ├── secret-management.md
│   │   ├── reference-grant.md
│   │   └── mtls.md
│   └── observability/
│       ├── access-log.md
│       └── metrics.md
└── dev-guide/
    ├── README.md
    ├── architecture-overview.md
    ├── resource-architecture-overview.md
    ├── resource-registry-guide.md
    ├── add-new-resource-guide.md
    ├── annotations-guide.md
    ├── http-plugin-development.md
    ├── stream-plugin-development.md
    ├── work-directory.md
    ├── logging-system.md
    ├── ci-release-workflow.md
    ├── ai-agent-collaboration.md
    ├── knowledge-source-map.md
    └── jwt-auth-plugin-design.md
```

## zh-CN/ — 中文

```
zh-CN/
├── README.md
├── getting-started/
│   ├── README.md
│   ├── installation-and-deployment.md
│   ├── first-gateway.md
│   └── core-concepts.md
├── user-guide/
│   ├── README.md
│   ├── http-route/
│   │   ├── README.md
│   │   ├── overview.md
│   │   ├── lb-algorithms.md
│   │   ├── matches/
│   │   │   ├── README.md, path.md, headers.md, query-params.md, method.md
│   │   ├── filters/
│   │   │   ├── README.md, overview.md, plugin-composition.md
│   │   │   ├── gateway-api/  (5 files, same as en/)
│   │   │   └── edgion-plugins/  (README.md + 28 plugin files, same as en/)
│   │   ├── backends/
│   │   │   ├── README.md, service-ref.md, weight.md, backend-tls.md, health-check.md, circuit-breaker.md
│   │   └── resilience/
│   │       ├── README.md, timeouts.md, retry.md, session-persistence.md
│   ├── grpc-route/  (README.md + same structure as en/)
│   ├── tcp-route/  (README.md + same structure as en/)
│   ├── udp-route/  (README.md + same structure as en/)
│   └── edgion-plugins/
│       ├── README.md
│       ├── real-ip.md, rate-limit.md, region-route.md, openid-connect.md
│       ├── ldap-auth.md, jwe-decrypt.md, forward-auth.md, ctx-setter.md
├── ops-guide/  (same structure as en/, gateway/ and configuration/ included)
├── promotion/                  # zh-CN 独有，推广材料
│   ├── README.md
│   ├── introduction.md
│   ├── whitepaper.md
│   └── .data-reference.md
└── dev-guide/
    ├── README.md, architecture-overview.md
    ├── resource-architecture-overview.md, resource-registry-guide.md
    ├── add-new-resource-guide.md, annotations-guide.md
    ├── http-plugin-development.md
    ├── stream-plugin-development.md
    ├── work-directory.md, logging-system.md
    ├── resource-abnormal-metric.md
    ├── ci-release-workflow.md
    ├── ai-agent-collaboration.md, knowledge-source-map.md
    └── jwt-auth-plugin-design.md
```

## ja/ — 日本語

```
ja/
└── README.md   # Placeholder, references zh-CN/
```

## refactoring/

```
refactoring/
└── conf_server_processor_refactoring_spec.md
```

## review/

```
review/
├── hostname-refactor-review.md
├── hostname-resolution-refactor-review.md
└── openid-connect-review-card.md
```
