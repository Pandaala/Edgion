# Edgion Gateway Curl 测试命令

> 网关默认监听 `http://127.0.0.1:18080`

## 规则1: /aaa/* - 规则级别 ExtensionRef

```bash
# 测试 /aaa/doc/ (PathPrefix)
curl -k -v -H "Host: aaa.example.com" http://127.0.0.1:18080/aaa/doc/test.html

# 测试 /aaa/exact (PathPrefix)
curl -k -v -H "Host: aaa.example.com" http://127.0.0.1:18080/aaa/exact/path

# 只看响应头
curl -k -I -H "Host: aaa.example.com" http://127.0.0.1:18080/aaa/doc/
```

## 规则2: /bbb/* - 后端级别 ExtensionRef

```bash
# 测试 /bbb/123 (PathPrefix)
curl -k -v -H "Host: bbb.example.com" http://127.0.0.1:18080/bbb/123/test

# 测试路径参数 /bbb/{id1}/ccc/{id2}/eee
curl -k -v -H "Host: bbb.example.com" http://127.0.0.1:18080/bbb/user001/ccc/order999/eee
```

## 规则3: /ccc/* - 混合使用 filter + ExtensionRef

```bash
# 测试 /ccc/api/ (应同时应用 X-Rule-Level header 和 EdgionPlugins)
curl -k -v -H "Host: aaa.example.com" http://127.0.0.1:18080/ccc/api/v1/users

# 测试路径参数 /ccc/{id1}/{id2}/ddd
curl -k -v -H "Host: aaa.example.com" http://127.0.0.1:18080/ccc/abc/xyz/ddd

# 只看响应头
curl -k -I -H "Host: aaa.example.com" http://127.0.0.1:18080/ccc/api/
```

## 预期响应头 (由 EdgionPlugins 添加)

响应应包含以下头 (由 `common-plugins` EdgionPlugins 配置):

```
X-Content-Type-Options: nosniff
X-Frame-Options: DENY
X-Powered-By: Edgion
```

请求头应被修改 (后端可见):
```
X-Request-Id: {{uuid}}
X-Forwarded-By: edgion-gateway
X-Custom-Header: custom-value
```

规则3 还会额外添加:
```
X-Rule-Level: true
```

## 错误测试

```bash
# 不存在的路径 (应返回 404)
curl -k -v -H "Host: aaa.example.com" http://127.0.0.1:18080/not-exist/path

# 错误的 Host
curl -k -v -H "Host: unknown.example.com" http://127.0.0.1:18080/aaa/doc/
```

## 快速批量测试

```bash
# 运行测试脚本
chmod +x tests/curl_test.sh
./tests/curl_test.sh
```

