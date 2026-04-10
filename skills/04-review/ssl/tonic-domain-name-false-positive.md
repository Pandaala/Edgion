# tonic ClientTlsConfig 不调用 domain_name() 不是漏洞

## 结论

`tonic::transport::ClientTlsConfig` 不显式调用 `.domain_name()` **不构成 MITM 风险**。

## 原因

- tonic 默认从 `Endpoint` URI 的 host 部分提取 hostname，用于 TLS 握手中的 SNI 和证书 SAN/CN 校验
- Edgion 的 `build_grpc_uri()` 构建 `https://hostname:port` 形式的 URI，hostname 来自用户配置的连接地址
- K8s 环境下连接地址使用 Service DNS 名（如 `edgion-controller.ns.svc`），证书 SAN 与之匹配，校验自动通过
- 证书由项目自己的 `generate_certs()` 生成，CA 完全自控，不存在"同一 CA 签发的其他服务证书泄露"的场景

## 判定规则

当 review 报告"TLS 客户端未调用 `domain_name()` 导致 MITM 风险"时，检查：

1. Endpoint URI 是否包含正确的 hostname（非裸 IP）→ 是则 tonic 默认行为已足够
2. 证书 CA 是否自控 → 是则不存在"第三方同 CA 证书"攻击面
3. 连接地址是否由运维配置 → 是则配置正确性由运维保证

三条都满足 → 非真实问题，关闭。
