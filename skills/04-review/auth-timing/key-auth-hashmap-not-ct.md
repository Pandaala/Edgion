# Key Auth HashMap 查找非 constant-time：非真实攻击面

## 结论

`key_auth` 插件用 `HashMap<[u8;32], KeyMetadata>::get(&sha256_of(key))` 查找 API key，HashMap 本身不是常数时间、`[u8;32]` 比较也会短路。**此非真实攻击面**，不接受"改线性扫描 + `subtle::ConstantTimeEq`"类修复建议。

## 原因

1. **输入先过 SHA-256**：攻击者控制的是 `key_candidate` 原串，SHA-256 输出是伪随机的，无法构造"在第 N 字节处差异可控"的候选来做分字节时序推断
2. **HashMap 时序泄露量极小且无价值**：最多泄露"sha256 输出头几 byte 的桶命中相似度"，但 SHA-256 单向不可逆，这个信息对恢复原始 key 无任何帮助
3. **401 响应已是二元 oracle**：`validate_key` 返回 None → 401、返回 Some → 200；"key 是否在表中"直接从状态码就能判断，时序 oracle 不增加攻击者的信息收获
4. **经典分字节时序侧信道不成立**：SHA-256 预处理摊平了输入差异，"改一字节看匹配多几位"的攻击立足点不存在
5. **同类网关一致做法**：Kong、APISIX、Tyk、Traefik 等主流 API 网关的 key 查找同样是 HashMap / etcd 查找，无一做 CT 线性扫描

## 判定规则

当 review / 审计报告出现以下模式时，按"非真实问题"关闭：

1. "key_auth HashMap::get 非 constant-time，存在时序侧信道"
2. "建议改用 `subtle::ConstantTimeEq` 或线性扫描所有哈希做 CT 比较"
3. "[u8;32] == 会短路，泄露匹配字节数"

验证两点再关闭：

- `key_auth/plugin.rs` 的 `validate_key` 仍走 `self.keys.get(&sha256_of(key))`（非线性扫描）
- 输入 key 确实先经 SHA-256（未改成直接存明文 key）

两点满足 → 非真实问题，指向 `tasks/security-audit/01-auth-plugins.md` 的 M10 NOT-AN-ISSUE 决策，关闭。

## 例外场景（仅在极端威胁模型下需重评）

如果未来在"攻击者可精确测量本地 / 旁路通道纳秒级时序"的场景下部署（如公有硬件 / SGX / 租户共享内核），这个结论需要重评。普通 K8s 部署不在此列。

## 背景记录

- 决策日期：2026-04-21
- 决策来源：`tasks/security-audit/` M10 条目
- 对比文件：`auth-timing/basic-auth-username-enumeration.md`（M4，同样是"网关层反时序防护主流不做"的模式；但 M4 有实际可利用性，走 doc-only；M10 无实际可利用性，走 NOT-AN-ISSUE）
