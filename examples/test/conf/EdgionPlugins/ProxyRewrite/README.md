# ProxyRewrite 插件集成测试

## 文件结构

```
ProxyRewrite/
├── EdgionPlugins_default_proxy-rewrite.yaml  # 插件配置 (所有场景)
├── HTTPRoute_default_proxy-rewrite.yaml      # 路由配置 (所有场景)
└── README.md
```

## 测试场景

### 1. URI 重写 (`/uri/*`)

| 路径 | 测试内容 |
|------|----------|
| `/uri/simple/*` | 简单替换为固定值 |
| `/uri/var/*` | 使用 `$uri` 变量 |
| `/uri/arg/*` | 使用 `$arg_xxx` 变量 |

### 2. Regex URI 重写 (`/regex/*`)

| 路径 | 测试内容 |
|------|----------|
| `/regex/users/:id` | 单捕获组 `$1` |
| `/regex/api/:type/:id/:action` | 多捕获组 |
| `/regex/profile/:id` | 捕获组用于 Header |

### 3. Host/Method 重写

| 路径 | 测试内容 |
|------|----------|
| `/host/rewrite/*` | Host 重写 |
| `/method/to-post/*` | GET -> POST |
| `/combo/full/*` | URI + Host + Method |

### 4. Headers 操作 (`/headers/*`)

| 路径 | 测试内容 |
|------|----------|
| `/headers/add/*` | 添加 Header |
| `/headers/set/*` | 设置 Header |
| `/headers/remove/*` | 删除 Header |
| `/headers/combo/*` | add + set + remove |

### 5. 路径参数变量 (`/params/*`)

| 路由 Pattern | 测试内容 |
|--------------|----------|
| `/params/uri/:uid` | `$uid` 用于 URI |
| `/params/header/:uid/:action` | 多参数用于 Header |
| `/params/mixed/:service/:resource` | 路径参数 + Query 参数 |

### 6. 综合测试 (`/full/*`)

| 路由 Pattern | 测试内容 |
|--------------|----------|
| `/full/api/:uid` | 完整 API 网关重写 |
| `/full/query/*` | Query String 保留 |

## 变量支持

| 变量 | 说明 | 示例 |
|------|------|------|
| `$uri` | 原始请求路径 | `/api/v1/users` |
| `$arg_<name>` | Query 参数 | `$arg_keyword` |
| `$1-$9` | Regex 捕获组 | `$1`, `$2` |
| `$<name>` | 路径参数 | `$uid`, `$service` |

## 测试命令

```bash
# Host: proxy-rewrite.example.com

# URI 重写
curl -H "Host: proxy-rewrite.example.com" http://localhost:31180/uri/simple/test
curl -H "Host: proxy-rewrite.example.com" http://localhost:31180/uri/arg/test?keyword=hello&lang=en

# Regex 重写
curl -H "Host: proxy-rewrite.example.com" http://localhost:31180/regex/users/123

# 路径参数
curl -H "Host: proxy-rewrite.example.com" http://localhost:31180/params/uri/456/data
curl -H "Host: proxy-rewrite.example.com" http://localhost:31180/params/header/789/edit

# 综合测试
curl -H "Host: proxy-rewrite.example.com" http://localhost:31180/full/api/999/profile?trace_id=abc123
```
