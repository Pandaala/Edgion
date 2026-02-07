# RateLimit 插件集成测试

## 测试概述

RateLimit 插件使用 Pingora 的 Count-Min Sketch (CMS) 算法实现高性能限流。

## 测试配置

### 限流配置
- **Rate**: 5 请求
- **Interval**: 10 秒窗口
- **Key Source**: Header (`X-Rate-Key`)
- **Reject Status**: 429

### 测试路由
- **Host**: `rate-limit.example.com`
- **Path**: `/test/rate-limit/*`
- **Gateway Port**: 31180

## 测试场景

| 测试名 | 描述 | 预期结果 |
|--------|------|----------|
| `allows_within_limit` | 在限制内的请求 | 200 OK |
| `blocks_over_limit` | 超过限制的请求 | 429 Too Many Requests |
| `headers_present` | 验证限流响应头 | X-RateLimit-* headers |
| `different_keys_independent` | 不同 key 独立限流 | 各自独立计数 |

## 运行测试

```bash
# 运行 RateLimit 测试套件
cargo run --bin test-client -- --suite rate-limit
```

## 注意事项

1. 测试使用唯一的 `X-Rate-Key` 值避免测试间干扰
2. 10 秒窗口设计用于避免测试超时
3. CMS 算法可能有轻微的计数误差（正常现象）
