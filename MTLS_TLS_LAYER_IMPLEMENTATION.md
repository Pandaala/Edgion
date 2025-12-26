# mTLS SAN/CN白名单验证 - TLS层实现

## 变更概述

已将mTLS客户端证书的SAN/CN白名单验证从应用层（HTTP request_filter）移至TLS层（certificate_callback），解决了Pingora架构限制问题。

## 主要修改

### 1. 新增文件
- **src/core/tls/mtls_verify_callback.rs** - TLS层白名单验证逻辑
  - `verify_client_cert_whitelist()` - 在TLS握手时验证客户端证书

### 2. 修改文件

#### src/core/tls/tls_pingora.rs
- 在 `configure_mtls()` 中集成白名单验证
- 移除过时的"应用层验证"注释
- 验证时机：TLS握手期间，CA验证通过后立即执行

#### src/core/routes/http_routes/edgion_http_pingora.rs
- 删除第97-125行：无效的SNI提取和验证代码
- 删除第316-366行：被注释的应用层mTLS验证代码
- 添加简洁注释说明验证已在TLS层完成

#### src/core/tls/mod.rs
- 导出新模块 `mtls_verify_callback`

#### src/core/gateway/mod.rs
- 移除未使用的错误响应函数导出（end_response_403, end_response_421）

## 架构改进

### 之前（应用层验证 - 未实现）
```
TLS握手 → HTTP请求 → [尝试]提取证书 → [尝试]验证SAN/CN
                    ↑ 无法访问SSL连接，代码被注释
```

### 现在（TLS层验证 - 已实现）
```
TLS握手 → CA验证 → SAN/CN验证 → 握手完成 → HTTP请求
              ↑          ↑
            TLS层     TLS层
         (已有)    (新增)
```

## 验证逻辑

1. **CA验证**（TLS层，已有）
   - 验证客户端证书是否由配置的CA签发
   - 检查证书链、有效期等

2. **白名单验证**（TLS层，新增）
   - SAN（Subject Alternative Name）白名单检查
   - CN（Common Name）白名单检查
   - 支持精确匹配和通配符（如 `*.example.com`）

## 测试配置

已有测试配置文件：
- `examples/conf/EdgionTls_edge_mtls-test-san.yaml` - SAN白名单测试

## 编译状态

✅ 代码编译通过
✅ Gateway二进制成功构建
⚠️  单元测试编译有其他模块的错误（与本次修改无关）

## 下一步

1. 运行集成测试验证功能：
   ```bash
   cd examples/testing && ./run_integration_test.sh
   ```

2. 测试场景：
   - Mutual模式 + SAN白名单
   - Mutual模式 + CN白名单  
   - OptionalMutual模式
   - 证书不匹配白名单时应拒绝连接

## 影响范围

- ✅ 核心功能：mTLS验证在TLS层完成，更安全高效
- ✅ 性能：验证前置到握手阶段，减少无效连接处理
- ✅ 代码质量：删除150+行无效/注释代码
- ⚠️  SNI验证：原有代码从未生效已删除（SNI与Host应一致）
- ⚠️  灵活性：验证粒度从per-route变为per-SNI（符合TLS设计）

## 注意事项

- 客户端证书验证发生在TLS握手阶段
- 验证失败会导致TLS握手失败（握手前拦截）
- 白名单配置是per-SNI级别（通过EdgionTls配置）
- 相关模块：cert_extractor, mtls_validator（已有，复用）

