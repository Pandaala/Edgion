# 12 - Serverless Integration (Serverless 集成)

**优先级**: P2 - 锦上添花

## 问题

将 FaaS 函数作为路由后端，无需独立部署服务。适合轻量级 API 和事件驱动场景。

## 竞品对比

| 网关 | 实现方式 |
|------|---------|
| APISIX | `aws-lambda`、`azure-functions`、`openwhisk` 插件 |
| Kong | AWS Lambda、Azure Functions 插件 |
| Envoy | 无原生支持（通过自定义 filter 或控制面路由实现） |

## 需要实现

- [ ] AWS Lambda 集成（IAM 签名、invoke API）
- [ ] Azure Functions 集成
- [ ] 通用 HTTP Function 后端（支持 OpenFaaS、Knative 等）
- [ ] 请求/响应格式转换（API Gateway event format ↔ HTTP）
- [ ] 作为 HTTP 插件或 backend 类型实现
