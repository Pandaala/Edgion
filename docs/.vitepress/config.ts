import { defineConfig } from 'vitepress'

// Nav route paths shared between locales — change here to keep both nav bars in sync
const PATHS = {
  gettingStarted: 'getting-started/installation-and-deployment',
  userGuide:      'user-guide/http-route/overview',
  opsGuide:       'ops-guide/gateway/overview',
  devGuide:       'dev-guide/architecture-overview',
}

const enSidebar = [
  {
    text: 'Getting Started',
    items: [
      { text: 'Installation & Deployment', link: '/en/getting-started/installation-and-deployment' },
      { text: 'Your First Gateway', link: '/en/getting-started/first-gateway' },
      { text: 'Core Concepts', link: '/en/getting-started/core-concepts' },
    ],
  },
  {
    text: 'User Guide',
    items: [
      {
        text: 'HTTP Route',
        collapsed: false,
        items: [
          { text: 'Overview', link: '/en/user-guide/http-route/overview' },
          {
            text: 'Backends',
            collapsed: true,
            items: [
              { text: 'Service Reference', link: '/en/user-guide/http-route/backends/service-ref' },
              { text: 'Weight', link: '/en/user-guide/http-route/backends/weight' },
              { text: 'Health Check', link: '/en/user-guide/http-route/backends/health-check' },
              { text: 'Circuit Breaker', link: '/en/user-guide/http-route/backends/circuit-breaker' },
              { text: 'Backend TLS', link: '/en/user-guide/http-route/backends/backend-tls' },
            ],
          },
          {
            text: 'Matches',
            collapsed: true,
            items: [
              { text: 'Path', link: '/en/user-guide/http-route/matches/path' },
              { text: 'Method', link: '/en/user-guide/http-route/matches/method' },
              { text: 'Headers', link: '/en/user-guide/http-route/matches/headers' },
              { text: 'Query Parameters', link: '/en/user-guide/http-route/matches/query-params' },
            ],
          },
          {
            text: 'Filters',
            collapsed: false,
            items: [
              { text: 'Overview', link: '/en/user-guide/http-route/filters/overview' },
              { text: 'Plugin Composition', link: '/en/user-guide/http-route/filters/plugin-composition' },
              {
                text: 'Edgion Plugins',
                collapsed: true,
                items: [
                  { text: 'Basic Auth', link: '/en/user-guide/http-route/filters/edgion-plugins/basic-auth' },
                  { text: 'JWT Auth', link: '/en/user-guide/http-route/filters/edgion-plugins/jwt-auth' },
                  { text: 'Key Auth', link: '/en/user-guide/http-route/filters/edgion-plugins/key-auth' },
                  { text: 'HMAC Auth', link: '/en/user-guide/http-route/filters/edgion-plugins/hmac-auth' },
                  { text: 'LDAP Auth', link: '/en/user-guide/http-route/filters/edgion-plugins/ldap-auth' },
                  { text: 'Header Cert Auth', link: '/en/user-guide/http-route/filters/edgion-plugins/header-cert-auth' },
                  { text: 'OpenID Connect', link: '/en/user-guide/http-route/filters/edgion-plugins/openid-connect' },
                  { text: 'Forward Auth', link: '/en/user-guide/http-route/filters/edgion-plugins/forward-auth' },
                  { text: 'JWE Decrypt', link: '/en/user-guide/http-route/filters/edgion-plugins/jwe-decrypt' },
                  { text: 'IP Restriction', link: '/en/user-guide/http-route/filters/edgion-plugins/ip-restriction' },
                  { text: 'Rate Limit', link: '/en/user-guide/http-route/filters/edgion-plugins/rate-limit' },
                  { text: 'Bandwidth Limit', link: '/en/user-guide/http-route/filters/edgion-plugins/bandwidth-limit' },
                  { text: 'Proxy Rewrite', link: '/en/user-guide/http-route/filters/edgion-plugins/proxy-rewrite' },
                  { text: 'Response Rewrite', link: '/en/user-guide/http-route/filters/edgion-plugins/response-rewrite' },
                  { text: 'Request Restriction', link: '/en/user-guide/http-route/filters/edgion-plugins/request-restriction' },
                  { text: 'CORS', link: '/en/user-guide/http-route/filters/edgion-plugins/cors' },
                  { text: 'CSRF', link: '/en/user-guide/http-route/filters/edgion-plugins/csrf' },
                  { text: 'DSL', link: '/en/user-guide/http-route/filters/edgion-plugins/dsl/', collapsed: true, items: [
                    { text: 'Language Syntax', link: '/en/user-guide/http-route/filters/edgion-plugins/dsl/language-syntax' },
                    { text: 'API Reference', link: '/en/user-guide/http-route/filters/edgion-plugins/dsl/api-reference' },
                    { text: 'Sandbox Security', link: '/en/user-guide/http-route/filters/edgion-plugins/dsl/sandbox-security' },
                    { text: 'Examples', link: '/en/user-guide/http-route/filters/edgion-plugins/dsl/examples' },
                    { text: 'Bytecode Guide', link: '/en/user-guide/http-route/filters/edgion-plugins/dsl/bytecode' },
                  ]},
                  { text: 'Mock', link: '/en/user-guide/http-route/filters/edgion-plugins/mock' },
                  { text: 'Dynamic Upstream', link: '/en/user-guide/http-route/filters/edgion-plugins/dynamic-upstream' },
                  { text: 'Direct Endpoint', link: '/en/user-guide/http-route/filters/edgion-plugins/direct-endpoint' },
                  { text: 'Request Mirror', link: '/en/user-guide/http-route/filters/edgion-plugins/request-mirror' },
                  { text: 'All Endpoint Status', link: '/en/user-guide/http-route/filters/edgion-plugins/all-endpoint-status' },
                  { text: 'Debug Access Log', link: '/en/user-guide/http-route/filters/edgion-plugins/debug-access-log' },
                  { text: 'Ctx Setter', link: '/en/user-guide/http-route/filters/edgion-plugins/ctx-setter' },
                  { text: 'Real IP', link: '/en/user-guide/http-route/filters/edgion-plugins/real-ip' },
                  { text: 'Region Route', link: '/en/user-guide/http-route/filters/edgion-plugins/region-route' },
                ],
              },
              {
                text: 'Gateway API Filters',
                collapsed: true,
                items: [
                  { text: 'Request Header Modifier', link: '/en/user-guide/http-route/filters/gateway-api/request-header-modifier' },
                  { text: 'Response Header Modifier', link: '/en/user-guide/http-route/filters/gateway-api/response-header-modifier' },
                  { text: 'Request Mirror', link: '/en/user-guide/http-route/filters/gateway-api/request-mirror' },
                  { text: 'Request Redirect', link: '/en/user-guide/http-route/filters/gateway-api/request-redirect' },
                  { text: 'URL Rewrite', link: '/en/user-guide/http-route/filters/gateway-api/url-rewrite' },
                ],
              },
            ],
          },
          {
            text: 'Resilience',
            collapsed: true,
            items: [
              { text: 'Retry', link: '/en/user-guide/http-route/resilience/retry' },
              { text: 'Timeouts', link: '/en/user-guide/http-route/resilience/timeouts' },
              { text: 'Session Persistence', link: '/en/user-guide/http-route/resilience/session-persistence' },
            ],
          },
          { text: 'Load Balancing', link: '/en/user-guide/http-route/lb-algorithms' },
        ],
      },
      {
        text: 'gRPC Route',
        collapsed: true,
        items: [
          { text: 'Overview', link: '/en/user-guide/grpc-route/overview' },
          { text: 'Backends', link: '/en/user-guide/grpc-route/backends/overview' },
          { text: 'Matches', link: '/en/user-guide/grpc-route/matches/overview' },
          { text: 'Filters', link: '/en/user-guide/grpc-route/filters/overview' },
        ],
      },
      {
        text: 'TCP Route',
        collapsed: true,
        items: [
          { text: 'Overview', link: '/en/user-guide/tcp-route/overview' },
          { text: 'Backends', link: '/en/user-guide/tcp-route/backends/overview' },
          { text: 'Stream Plugins', link: '/en/user-guide/tcp-route/stream-plugins' },
        ],
      },
      {
        text: 'UDP Route',
        collapsed: true,
        items: [
          { text: 'Overview', link: '/en/user-guide/udp-route/overview' },
          { text: 'Backends', link: '/en/user-guide/udp-route/backends/overview' },
        ],
      },
      {
        text: 'TLS Route',
        collapsed: true,
        items: [
          { text: 'Overview', link: '/en/user-guide/tls-route/overview' },
          { text: 'Backends', link: '/en/user-guide/tls-route/backends/overview' },
        ],
      },
      {
        text: 'Edgion Plugins (Route-Level)',
        collapsed: true,
        items: [
          { text: 'Overview', link: '/en/user-guide/edgion-plugins/' },
          { text: 'Ctx Setter', link: '/en/user-guide/edgion-plugins/ctx-setter' },
          { text: 'Forward Auth', link: '/en/user-guide/edgion-plugins/forward-auth' },
          { text: 'JWE Decrypt', link: '/en/user-guide/edgion-plugins/jwe-decrypt' },
          { text: 'LDAP Auth', link: '/en/user-guide/edgion-plugins/ldap-auth' },
          { text: 'OpenID Connect', link: '/en/user-guide/edgion-plugins/openid-connect' },
          { text: 'Rate Limit', link: '/en/user-guide/edgion-plugins/rate-limit' },
          { text: 'Real IP', link: '/en/user-guide/edgion-plugins/real-ip' },
          { text: 'Region Route', link: '/en/user-guide/edgion-plugins/region-route' },
        ],
      },
    ],
  },
  {
    text: 'Ops Guide',
    items: [
      {
        text: 'Gateway',
        collapsed: false,
        items: [
          { text: 'Overview', link: '/en/ops-guide/gateway/overview' },
          { text: 'GatewayClass', link: '/en/ops-guide/gateway/gateway-class' },
          { text: 'HTTP to HTTPS Redirect', link: '/en/ops-guide/gateway/http-to-https-redirect' },
          { text: 'Preflight Policy', link: '/en/ops-guide/gateway/preflight-policy' },
          {
            text: 'Listeners',
            collapsed: true,
            items: [
              { text: 'HTTP', link: '/en/ops-guide/gateway/listeners/http' },
              { text: 'HTTPS', link: '/en/ops-guide/gateway/listeners/https' },
              { text: 'gRPC', link: '/en/ops-guide/gateway/listeners/grpc' },
              { text: 'TCP', link: '/en/ops-guide/gateway/listeners/tcp' },
            ],
          },
          {
            text: 'TLS',
            collapsed: true,
            items: [
              { text: 'TLS Termination', link: '/en/ops-guide/gateway/tls/tls-termination' },
              { text: 'Edgion TLS', link: '/en/ops-guide/gateway/tls/edgion-tls' },
              { text: 'ACME', link: '/en/ops-guide/gateway/tls/acme' },
            ],
          },
        ],
      },
      {
        text: 'Observability',
        collapsed: true,
        items: [
          { text: 'Access Log', link: '/en/ops-guide/observability/access-log' },
          { text: 'Metrics', link: '/en/ops-guide/observability/metrics' },
        ],
      },
      {
        text: 'Infrastructure',
        collapsed: true,
        items: [
          { text: 'mTLS', link: '/en/ops-guide/infrastructure/mtls' },
          { text: 'Reference Grant', link: '/en/ops-guide/infrastructure/reference-grant' },
          { text: 'Secret Management', link: '/en/ops-guide/infrastructure/secret-management' },
          { text: 'Connectors (LinkSys)', link: '/en/ops-guide/infrastructure/connectors' },
        ],
      },
      {
        text: 'Configuration Reference',
        collapsed: true,
        items: [
          { text: 'Controller Config', link: '/en/ops-guide/configuration/controller-config' },
          { text: 'Gateway Config', link: '/en/ops-guide/configuration/gateway-config' },
          { text: 'EdgionGatewayConfig CRD', link: '/en/ops-guide/configuration/edgion-gateway-config' },
        ],
      },
      { text: 'Federation & RBAC', link: '/en/ops-guide/federation-rbac' },
      { text: 'Edgion CLI', link: '/en/ops-guide/edgion-cli' },
    ],
  },
  {
    text: 'Dev Guide',
    collapsed: true,
    items: [
      { text: 'Architecture Overview', link: '/en/dev-guide/architecture-overview' },
      { text: 'Resource Architecture', link: '/en/dev-guide/resource-architecture-overview' },
      { text: 'HTTP Plugin Development', link: '/en/dev-guide/http-plugin-development' },
      { text: 'Stream Plugin Development', link: '/en/dev-guide/stream-plugin-development' },
      { text: 'JWT Auth Plugin Design', link: '/en/dev-guide/jwt-auth-plugin-design' },
      { text: 'Add New Resource', link: '/en/dev-guide/add-new-resource-guide' },
      { text: 'Annotations Guide', link: '/en/dev-guide/annotations-guide' },
      { text: 'Logging System', link: '/en/dev-guide/logging-system' },
      { text: 'Work Directory', link: '/en/dev-guide/work-directory' },
      { text: 'CI & Release', link: '/en/dev-guide/ci-release-workflow' },
    ],
  },
]

// ── Chinese sidebar ────────────────────────────────────────────────────────

const zhSidebar = [
  {
    text: '快速开始',
    items: [
      { text: '安装与部署', link: '/zh-CN/getting-started/installation-and-deployment' },
      { text: '第一个网关', link: '/zh-CN/getting-started/first-gateway' },
      { text: '核心概念', link: '/zh-CN/getting-started/core-concepts' },
    ],
  },
  {
    text: '用户指南',
    items: [
      {
        text: 'HTTP 路由',
        collapsed: false,
        items: [
          { text: '概览', link: '/zh-CN/user-guide/http-route/overview' },
          {
            text: '后端',
            collapsed: true,
            items: [
              { text: '服务引用', link: '/zh-CN/user-guide/http-route/backends/service-ref' },
              { text: '权重', link: '/zh-CN/user-guide/http-route/backends/weight' },
              { text: '健康检查', link: '/zh-CN/user-guide/http-route/backends/health-check' },
              { text: '熔断器', link: '/zh-CN/user-guide/http-route/backends/circuit-breaker' },
              { text: '后端 TLS', link: '/zh-CN/user-guide/http-route/backends/backend-tls' },
            ],
          },
          {
            text: '匹配规则',
            collapsed: true,
            items: [
              { text: '路径', link: '/zh-CN/user-guide/http-route/matches/path' },
              { text: '方法', link: '/zh-CN/user-guide/http-route/matches/method' },
              { text: '请求头', link: '/zh-CN/user-guide/http-route/matches/headers' },
              { text: '查询参数', link: '/zh-CN/user-guide/http-route/matches/query-params' },
            ],
          },
          {
            text: '过滤器',
            collapsed: false,
            items: [
              { text: '概览', link: '/zh-CN/user-guide/http-route/filters/overview' },
              { text: '插件组合', link: '/zh-CN/user-guide/http-route/filters/plugin-composition' },
              {
                text: 'Edgion 插件',
                collapsed: true,
                items: [
                  { text: 'Basic Auth', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/basic-auth' },
                  { text: 'JWT Auth', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/jwt-auth' },
                  { text: 'Key Auth', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/key-auth' },
                  { text: 'HMAC Auth', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/hmac-auth' },
                  { text: 'LDAP Auth', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/ldap-auth' },
                  { text: 'Header Cert Auth', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/header-cert-auth' },
                  { text: 'OpenID Connect', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/openid-connect' },
                  { text: 'Forward Auth', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/forward-auth' },
                  { text: 'JWE Decrypt', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/jwe-decrypt' },
                  { text: 'IP 限制', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/ip-restriction' },
                  { text: '限流', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/rate-limit' },
                  { text: '带宽限制', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/bandwidth-limit' },
                  { text: '代理重写', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/proxy-rewrite' },
                  { text: '响应重写', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/response-rewrite' },
                  { text: '请求限制', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/request-restriction' },
                  { text: 'CORS', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/cors' },
                  { text: 'CSRF', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/csrf' },
                  { text: 'DSL', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/dsl/', collapsed: true, items: [
                    { text: '语言语法参考', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/dsl/language-syntax' },
                    { text: 'API 参考手册', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/dsl/api-reference' },
                    { text: '沙盒安全模型', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/dsl/sandbox-security' },
                    { text: '实战示例集', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/dsl/examples' },
                    { text: '字节码指南', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/dsl/bytecode' },
                  ]},
                  { text: 'Mock', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/mock' },
                  { text: '动态上游', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/dynamic-upstream' },
                  { text: '直接端点', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/direct-endpoint' },
                  { text: '请求镜像', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/request-mirror' },
                  { text: '全端点状态', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/all-endpoint-status' },
                  { text: '调试访问日志', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/debug-access-log' },
                  { text: 'Ctx Setter', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/ctx-setter' },
                  { text: 'Real IP', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/real-ip' },
                  { text: '区域路由', link: '/zh-CN/user-guide/http-route/filters/edgion-plugins/region-route' },
                ],
              },
              {
                text: 'Gateway API 过滤器',
                collapsed: true,
                items: [
                  { text: '请求头修改', link: '/zh-CN/user-guide/http-route/filters/gateway-api/request-header-modifier' },
                  { text: '响应头修改', link: '/zh-CN/user-guide/http-route/filters/gateway-api/response-header-modifier' },
                  { text: '请求镜像', link: '/zh-CN/user-guide/http-route/filters/gateway-api/request-mirror' },
                  { text: '请求重定向', link: '/zh-CN/user-guide/http-route/filters/gateway-api/request-redirect' },
                  { text: 'URL 重写', link: '/zh-CN/user-guide/http-route/filters/gateway-api/url-rewrite' },
                ],
              },
            ],
          },
          {
            text: '弹性',
            collapsed: true,
            items: [
              { text: '重试', link: '/zh-CN/user-guide/http-route/resilience/retry' },
              { text: '超时', link: '/zh-CN/user-guide/http-route/resilience/timeouts' },
              { text: '会话保持', link: '/zh-CN/user-guide/http-route/resilience/session-persistence' },
            ],
          },
          { text: '负载均衡', link: '/zh-CN/user-guide/http-route/lb-algorithms' },
        ],
      },
      {
        text: 'gRPC 路由',
        collapsed: true,
        items: [
          { text: '概览', link: '/zh-CN/user-guide/grpc-route/overview' },
          { text: '后端', link: '/zh-CN/user-guide/grpc-route/backends/overview' },
          { text: '匹配规则', link: '/zh-CN/user-guide/grpc-route/matches/overview' },
          { text: '过滤器', link: '/zh-CN/user-guide/grpc-route/filters/overview' },
        ],
      },
      {
        text: 'TCP 路由',
        collapsed: true,
        items: [
          { text: '概览', link: '/zh-CN/user-guide/tcp-route/overview' },
          { text: '后端', link: '/zh-CN/user-guide/tcp-route/backends/overview' },
          { text: '流插件', link: '/zh-CN/user-guide/tcp-route/stream-plugins' },
        ],
      },
      {
        text: 'UDP 路由',
        collapsed: true,
        items: [
          { text: '概览', link: '/zh-CN/user-guide/udp-route/overview' },
          { text: '后端', link: '/zh-CN/user-guide/udp-route/backends/overview' },
        ],
      },
      {
        text: 'Edgion 插件（路由级）',
        collapsed: true,
        items: [
          { text: '概览', link: '/zh-CN/user-guide/edgion-plugins/' },
          { text: 'Ctx Setter', link: '/zh-CN/user-guide/edgion-plugins/ctx-setter' },
          { text: 'Forward Auth', link: '/zh-CN/user-guide/edgion-plugins/forward-auth' },
          { text: 'JWE Decrypt', link: '/zh-CN/user-guide/edgion-plugins/jwe-decrypt' },
          { text: 'LDAP Auth', link: '/zh-CN/user-guide/edgion-plugins/ldap-auth' },
          { text: 'OpenID Connect', link: '/zh-CN/user-guide/edgion-plugins/openid-connect' },
          { text: '限流', link: '/zh-CN/user-guide/edgion-plugins/rate-limit' },
          { text: 'Real IP', link: '/zh-CN/user-guide/edgion-plugins/real-ip' },
          { text: '区域路由', link: '/zh-CN/user-guide/edgion-plugins/region-route' },
        ],
      },
    ],
  },
  {
    text: '运维指南',
    items: [
      {
        text: '网关',
        collapsed: false,
        items: [
          { text: '概览', link: '/zh-CN/ops-guide/gateway/overview' },
          { text: 'GatewayClass', link: '/zh-CN/ops-guide/gateway/gateway-class' },
          { text: 'HTTP 跳转 HTTPS', link: '/zh-CN/ops-guide/gateway/http-to-https-redirect' },
          { text: '预检策略', link: '/zh-CN/ops-guide/gateway/preflight-policy' },
          {
            text: '监听器',
            collapsed: true,
            items: [
              { text: 'HTTP', link: '/zh-CN/ops-guide/gateway/listeners/http' },
              { text: 'HTTPS', link: '/zh-CN/ops-guide/gateway/listeners/https' },
              { text: 'gRPC', link: '/zh-CN/ops-guide/gateway/listeners/grpc' },
              { text: 'TCP', link: '/zh-CN/ops-guide/gateway/listeners/tcp' },
            ],
          },
          {
            text: 'TLS',
            collapsed: true,
            items: [
              { text: 'TLS 终止', link: '/zh-CN/ops-guide/gateway/tls/tls-termination' },
              { text: 'Edgion TLS', link: '/zh-CN/ops-guide/gateway/tls/edgion-tls' },
              { text: 'ACME', link: '/zh-CN/ops-guide/gateway/tls/acme' },
            ],
          },
        ],
      },
      {
        text: '可观测性',
        collapsed: true,
        items: [
          { text: '访问日志', link: '/zh-CN/ops-guide/observability/access-log' },
          { text: '指标', link: '/zh-CN/ops-guide/observability/metrics' },
        ],
      },
      {
        text: '基础设施',
        collapsed: true,
        items: [
          { text: 'mTLS', link: '/zh-CN/ops-guide/infrastructure/mtls' },
          { text: '引用授权', link: '/zh-CN/ops-guide/infrastructure/reference-grant' },
          { text: '密钥管理', link: '/zh-CN/ops-guide/infrastructure/secret-management' },
        ],
      },
      { text: 'Edgion CLI', link: '/zh-CN/ops-guide/edgion-cli' },
    ],
  },
  {
    text: '开发指南',
    collapsed: true,
    items: [
      { text: '架构概览', link: '/zh-CN/dev-guide/architecture-overview' },
      { text: '资源架构', link: '/zh-CN/dev-guide/resource-architecture-overview' },
      { text: 'HTTP 插件开发', link: '/zh-CN/dev-guide/http-plugin-development' },
      { text: '流插件开发', link: '/zh-CN/dev-guide/stream-plugin-development' },
      { text: 'JWT Auth 插件设计', link: '/zh-CN/dev-guide/jwt-auth-plugin-design' },
      { text: '添加新资源', link: '/zh-CN/dev-guide/add-new-resource-guide' },
      { text: '注解指南', link: '/zh-CN/dev-guide/annotations-guide' },
      { text: '日志系统', link: '/zh-CN/dev-guide/logging-system' },
      { text: '工作目录', link: '/zh-CN/dev-guide/work-directory' },
      { text: 'CI & 发布', link: '/zh-CN/dev-guide/ci-release-workflow' },
    ],
  },
]

export default defineConfig({
  title: 'Edgion',
  description: 'High-performance Kubernetes Gateway built on Pingora and Gateway API',
  base: '/Edgion/',

  // Exclude internal / non-public directories from being processed
  srcExclude: [
    '**/refactoring/**',
    '**/review/**',
    '**/superpowers/**',
    '**/promotion/**',
  ],

  locales: {
    en: {
      label: 'English',
      lang: 'en-US',
      themeConfig: {
        nav: [
          { text: 'Getting Started', link: `/en/${PATHS.gettingStarted}` },
          { text: 'User Guide',      link: `/en/${PATHS.userGuide}` },
          { text: 'Ops Guide',       link: `/en/${PATHS.opsGuide}` },
          { text: 'Dev Guide',       link: `/en/${PATHS.devGuide}` },
        ],
        sidebar: enSidebar,
        outline: { label: 'On This Page' },
        docFooter: { prev: 'Previous', next: 'Next' },
      },
    },

    'zh-CN': {
      label: '中文',
      lang: 'zh-CN',
      themeConfig: {
        nav: [
          { text: '快速开始', link: `/zh-CN/${PATHS.gettingStarted}` },
          { text: '用户指南', link: `/zh-CN/${PATHS.userGuide}` },
          { text: '运维指南', link: `/zh-CN/${PATHS.opsGuide}` },
          { text: '开发指南', link: `/zh-CN/${PATHS.devGuide}` },
        ],
        sidebar: zhSidebar,
        outline: { label: '本页目录' },
        docFooter: { prev: '上一页', next: '下一页' },
      },
    },
  },

  themeConfig: {
    outline: { level: [2, 3] },
    socialLinks: [
      { icon: 'github', link: 'https://github.com/Pandaala/Edgion' },
    ],
    footer: {
      message: 'Released under the Apache 2.0 License.',
      copyright: 'Copyright © 2025-present Edgion Contributors',
    },
    search: {
      provider: 'local',
    },
  },
})
