# Edgion 文档目录树

> 自动维护的文档完整路径索引。修改文档结构后请同步更新。
> AI 可直接读取此文件定位任何文档。

## 顶层结构

```
docs/
├── README.md                       # 项目级 README（待修正）
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
│   └── README.md
├── user-guide/
│   ├── README.md
│   ├── http-route/
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
│   │   │   │   ├── request-redirect.md
│   │   │   │   └── url-rewrite.md
│   │   │   └── edgion-plugins/
│   │   │       ├── basic-auth.md
│   │   │       ├── cors.md
│   │   │       ├── csrf.md
│   │   │       ├── ip-restriction.md
│   │   │       ├── jwt-auth.md
│   │   │       ├── key-auth.md
│   │   │       ├── hmac-auth.md
│   │   │       ├── header-cert-auth.md
│   │   │       ├── proxy-rewrite.md
│   │   │       ├── request-restriction.md
│   │   │       ├── request-mirror.md
│   │   │       ├── rate-limit.md
│   │   │       ├── response-rewrite.md
│   │   │       ├── dynamic-upstream.md
│   │   │       ├── dsl.md
│   │   │       ├── direct-endpoint.md
│   │   │       ├── mock.md
│   │   │       └── bandwidth-limit.md
│   │   ├── backends/
│   │   │   ├── README.md
│   │   │   ├── service-ref.md
│   │   │   ├── weight.md
│   │   │   ├── backend-tls.md
│   │   │   └── health-check.md
│   │   └── resilience/
│   │       ├── README.md
│   │       ├── timeouts.md
│   │       ├── retry.md
│   │       └── session-persistence.md
│   ├── grpc-route/
│   │   ├── overview.md
│   │   ├── matches/overview.md
│   │   ├── filters/overview.md
│   │   └── backends/overview.md
│   ├── tcp-route/
│   │   ├── overview.md
│   │   ├── stream-plugins.md
│   │   └── backends/overview.md
│   ├── udp-route/
│   │   ├── overview.md
│   │   └── backends/overview.md
│   └── edgion-plugins/
│       ├── real-ip.md
│       ├── rate-limit.md
│       ├── openid-connect.md
│       ├── ldap-auth.md
│       ├── jwe-decrypt.md
│       ├── forward-auth.md
│       └── ctx-setter.md
├── ops-guide/
│   ├── README.md
│   ├── edgion-ctl.md
│   ├── gateway/
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
    ├── work-directory.md
    ├── logging-system.md
    └── jwt-auth-plugin-design.md
```

## zh-CN/ — 中文

```
zh-CN/
├── README.md
├── getting-started/
│   └── README.md
├── user-guide/
│   ├── README.md
│   ├── http-route/
│   │   ├── overview.md
│   │   ├── lb-algorithms.md
│   │   ├── matches/
│   │   │   ├── README.md, path.md, headers.md, query-params.md, method.md
│   │   ├── filters/
│   │   │   ├── README.md, overview.md, plugin-composition.md
│   │   │   ├── gateway-api/  (4 files, same as en/)
│   │   │   └── edgion-plugins/  (18 files, same as en/)
│   │   ├── backends/
│   │   │   ├── README.md, service-ref.md, weight.md, backend-tls.md, health-check.md
│   │   └── resilience/
│   │       ├── README.md, timeouts.md, retry.md, session-persistence.md
│   ├── grpc-route/  (same structure as en/)
│   ├── tcp-route/  (same structure as en/)
│   ├── udp-route/  (same structure as en/)
│   └── edgion-plugins/
│       ├── real-ip.md, rate-limit.md, openid-connect.md
│       ├── openid-connect-review-card.md
│       ├── ldap-auth.md, jwe-decrypt.md, forward-auth.md, ctx-setter.md
├── ops-guide/  (same structure as en/)
└── dev-guide/
    ├── README.md, architecture-overview.md
    ├── resource-architecture-overview.md, resource-registry-guide.md
    ├── add-new-resource-guide.md, annotations-guide.md
    ├── work-directory.md, logging-system.md
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
└── hostname-resolution-refactor-review.md
```
