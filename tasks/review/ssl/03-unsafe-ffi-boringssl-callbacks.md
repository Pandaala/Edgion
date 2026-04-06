# Review: BoringSSL TLS 回调中的脆弱 Unsafe FFI

**审查编号**: SSL-03  
**严重级别**: 高 (High)  
**审查日期**: 2026-03-30  
**审查范围**: BoringSSL ex_data 管理、原始指针转换、C 回调安全性

---

## 1. 问题概述

Edgion 网关的 TLS 层通过 BoringSSL FFI (Foreign Function Interface) 实现自定义证书回调和 mTLS 验证。该实现在多处使用了**无安全保障的指针类型强制转换**、**`std::mem::transmute`**、以及**手动 `Box` 内存管理**来在 Rust 安全类型与 BoringSSL C 原始指针之间进行桥接。

核心问题在于：这些 unsafe 操作依赖于上游库 (pingora / boring) 内部类型的内存布局假设，而这些假设**不属于公开 API 契约**，可能在依赖升级时无声地破坏。

---

## 2. 受影响的文件与 Unsafe 代码块

### 2.1 `src/core/gateway/tls/runtime/gateway/tls_pingora.rs`

#### (a) `SslRef` → `boring_sys::SSL*` 指针强转 (共 4 处)

| 行号 | 函数 | 转换模式 |
|------|------|---------|
| 126 | `store_ssl_ctx` | `ssl as *mut SslRef as *mut boring_sys::SSL` |
| 169 | `take_ssl_ctx` | `ssl as *const TlsRef as *const boring_sys::SSL` |
| 676 | `configure_gateway_frontend_mtls` | `ssl as *mut SslRef as *mut boring_sys::SSL` |
| 823 | `configure_ciphers` | `ssl as *mut SslRef as *mut boring_sys::SSL` |

`store_ssl_ctx` 中的典型模式：

```rust
let ssl_ptr = ssl as *mut SslRef as *mut boring_sys::SSL;
```

`take_ssl_ctx` 中混用了不同的类型：

```rust
let ssl_ptr = ssl as *const TlsRef as *const boring_sys::SSL;
```

注意 `SslRef` 来自 `pingora_core::tls::ssl`，而 `TlsRef` 来自 `pingora_core::protocols::tls`。这两个类型被**等同地**转换为 `boring_sys::SSL*`，但它们在 pingora 内部可能是不同的类型别名或 wrapper。

#### (b) `Box::from_raw` 手动内存管理 (共 3 处)

| 行号 | 函数 | 操作 |
|------|------|------|
| 109 | `free_ssl_ctx` | `Box::from_raw(ptr as *mut SslCtx)` — 释放 ex_data |
| 130 | `store_ssl_ctx` | `Box::from_raw(ptr as *mut SslCtx)` — 存储失败时回收 |
| 174 | `take_ssl_ctx` | `Box::from_raw(ptr as *mut SslCtx)` — 取回所有权 |

#### (c) `insecure_fallback_verify_cb` (行 670–675)

```rust
unsafe extern "C" fn insecure_fallback_verify_cb(
    _preverify_ok: std::os::raw::c_int,
    _ctx: *mut boring_sys::X509_STORE_CTX,
) -> std::os::raw::c_int {
    1 // Always accept
}
```

始终返回 1 (验证通过) 的回调，用于 `AllowInsecureFallback` 模式。

#### (d) ex_data 索引管理

使用 `AtomicI32` + `std::sync::Once` 管理全局 ex_data 索引 `SSL_CTX_EX_DATA_IDX`。`free_ssl_ctx` 作为释放回调注册到 BoringSSL。

### 2.2 `src/core/gateway/tls/boringssl/mtls_verify_callback.rs`

#### (a) `std::mem::transmute` (行 178)

```rust
let ssl_ref: &SslRef = std::mem::transmute(&*ssl_ptr);
```

将 `boring_sys::SSL*` 解引用后 transmute 为 `&SslRef` 引用。此转换的安全性完全依赖于 `SslRef` 是 `boring_sys::SSL` 的 `#[repr(transparent)]` 包装——这是 `foreign_types` crate 的实现细节，**不是公开保证**。

#### (b) C 回调 + `catch_unwind` (行 90–111)

```rust
unsafe extern "C" fn verify_callback(preverify_ok: c_int, x509_ctx: *mut boring_sys::X509_STORE_CTX) -> c_int {
    let result = std::panic::catch_unwind(|| verify_callback_impl(preverify_ok, x509_ctx));
    result.unwrap_or_else(|e| { /* ... */ 0 })
}
```

#### (c) 第二套独立的 ex_data 管理

使用独立的 `VERIFY_EX_DATA_IDX` 和 `INIT_ONCE` 管理 `VerifyContext` 的 ex_data 索引，与 `tls_pingora.rs` 中的 `SSL_CTX_EX_DATA_IDX` 完全独立。

---

## 3. 影响分析

### 3.1 未定义行为 (UB) 风险

| 风险 | 触发条件 | 影响 |
|------|---------|------|
| **`SslRef` 内存布局变化** | 升级 `boring` 或 `pingora` crate | 所有 `SslRef` ↔ `SSL*` 转换产生 UB：悬空指针、内存损坏 |
| **`TlsRef` ≠ `SslRef` 布局** | `TlsRef` 和 `SslRef` 的底层类型不一致 | `take_ssl_ctx` 中的 `TlsRef → SSL*` 转换产生 UB |
| **`transmute` 假设失效** | `SslRef` 不再是 `#[repr(transparent)]` | `verify_callback_impl` 中的引用产生 UB |
| **栈溢出穿越 FFI** | `verify_callback_impl` 中深层调用栈溢出 | `catch_unwind` 无法捕获，进程 abort 或 UB |
| **double-free** | `take_ssl_ctx` 取回后 `free_ssl_ctx` 再次被调用 | 内存损坏（代码中已通过置空 ex_data 处理，但依赖于操作原子性） |

**关键风险**：`ssl as *mut SslRef as *mut boring_sys::SSL` 这个转换链的正确性依赖于 `SslRef` 在内存中**恰好是** `boring_sys::SSL` 的 newtype wrapper 且布局为 `#[repr(transparent)]`。`boring` crate 使用 `foreign_types` 宏生成这些类型——这是一个实现细节，不属于 semver 兼容性保证的范围。

### 3.2 内存安全风险

1. **所有权转移竞态**：`take_ssl_ctx` 中先 `SSL_get_ex_data` 取得指针，再 `Box::from_raw` 获取所有权，最后 `SSL_set_ex_data(null)` 清除槽位。如果在 `Box::from_raw` 和 `SSL_set_ex_data(null)` 之间另一个线程（或信号处理器）访问同一 ex_data，可能产生 use-after-free。虽然 BoringSSL 对单个 SSL 对象的操作不是线程安全的，但这一假设应被**显式记录和验证**。

2. **ex_data 泄漏路径**：如果 `store_ssl_ctx` 成功但连接在 `certificate_callback` 返回后、`handshake_complete_callback` 之前由于内部错误被销毁（非正常路径），内存释放完全依赖于 BoringSSL 的 `free_ssl_ctx` 回调。如果 free 回调未被调用（例如 BoringSSL 的 bug 或特殊错误路径），将发生内存泄漏。

3. **`peek_tls_conn_meta` 的引用安全**：行 157 `let ctx = &*(ptr as *const SslCtx)` 创建了一个对堆上 `SslCtx` 的共享引用，然后 `clone()` 了 `meta`。如果并发有 `take_ssl_ctx` 在执行（虽然理论上不应该），会产生 data race。

### 3.3 安全性风险

**`insecure_fallback_verify_cb` (严重)**

```rust
unsafe extern "C" fn insecure_fallback_verify_cb(...) -> c_int {
    1 // Always accept
}
```

此回调**无条件接受所有客户端证书**，包括：
- 过期证书
- 被吊销的证书
- 自签名证书（不在信任链中）
- 错误的 CA 签发的证书

虽然该回调仅在 `AllowInsecureFallback` 模式下使用，但：
- 该模式名称暗示是"后备"，但实际行为是**完全绕过证书验证**
- 没有审计日志记录哪些连接通过了此后备路径
- 没有配置级别的警告或文档说明此模式的安全影响
- 攻击者可以通过配置注入启用此模式来绕过 mTLS

---

## 4. 根因分析

### 4.1 缺乏安全的 BoringSSL 抽象层

根本原因是 **pingora 框架未在其 TLS 回调 API 中提供完整的安全抽象**。具体表现为：

1. **`TlsAccept` trait 的回调参数类型不一致**：`certificate_callback` 接收 `&mut TlsRef`，而需要调用 `boring_sys` 函数时必须将其强转为 `*mut boring_sys::SSL`。这说明上游 API 没有提供 ex_data 操作的安全封装。

2. **C 回调中无法获得安全的 `SslRef`**：BoringSSL 的 `SSL_set_verify` 回调只提供 `X509_STORE_CTX*`，通过 `X509_STORE_CTX_get_ex_data` 获得 `SSL*` 后，必须用 `transmute` 恢复为 `SslRef`，因为 pingora 没有提供 `SSL* → SslRef` 的安全转换函数。

3. **两种不同的 "SSL 引用" 类型**：`pingora_core::tls::ssl::SslRef`（来自 boring crate）和 `pingora_core::protocols::tls::TlsRef`（pingora 自身定义）在代码中被互换使用为 `boring_sys::SSL*` 的安全包装。但没有编译期保证它们的布局一致。

### 4.2 ex_data 模式的固有复杂性

BoringSSL 的 `ex_data` 机制本质上是一个 `void*` 的键值存储，完全绕过了 Rust 的类型系统。每个 ex_data 使用点都需要：
- 全局索引管理（`AtomicI32` + `Once`）
- 手动 `Box::into_raw` / `Box::from_raw` 进行所有权转移
- 手动注册 free 回调防止泄漏
- 类型擦除后的强转（`*mut c_void` → `*mut T`）

当前代码维护了**两套独立的 ex_data 体系**（`SslCtx` 和 `VerifyContext`），增加了心智负担和出错概率。

---

## 5. 修复建议

### 5.1 [P0] 引入安全的指针转换层

创建一个集中的 unsafe 封装模块，将所有 `SslRef ↔ boring_sys::SSL*` 转换收敛到一处，并添加编译时断言验证布局假设：

```rust
// src/core/gateway/tls/boringssl/ssl_ffi.rs

use pingora_core::tls::ssl::SslRef;
use pingora_core::protocols::tls::TlsRef;

/// 编译时验证 SslRef 与 boring_sys::SSL 的大小一致
const _: () = {
    assert!(
        std::mem::size_of::<*const SslRef>() == std::mem::size_of::<*const boring_sys::SSL>(),
        "SslRef pointer size mismatch with boring_sys::SSL"
    );
};

/// 将 &mut SslRef 转换为 *mut boring_sys::SSL
///
/// # Safety
/// 调用者必须确保返回的指针不超出 SslRef 的生命周期。
/// 此函数假设 SslRef 是 boring_sys::SSL 的 #[repr(transparent)] wrapper。
#[inline]
pub(crate) unsafe fn ssl_ref_as_ptr(ssl: &mut SslRef) -> *mut boring_sys::SSL {
    ssl as *mut SslRef as *mut boring_sys::SSL
}

/// 将 *const boring_sys::SSL 转换为 &SslRef
///
/// # Safety
/// - ssl_ptr 必须是有效的、已对齐的、非 null 的 SSL 指针
/// - 返回的引用生命周期不超过 ssl_ptr 的有效期
#[inline]
pub(crate) unsafe fn ssl_ptr_to_ref<'a>(ssl_ptr: *const boring_sys::SSL) -> &'a SslRef {
    &*(ssl_ptr as *const SslRef)
}
```

**好处**：
- 所有 unsafe 转换集中在一个模块，便于审计
- 升级依赖时只需检查一处
- 编译时断言提供额外的安全网

### 5.2 [P0] 统一 ex_data 管理

将两套 ex_data 管理合并为一个泛型安全封装：

```rust
/// 类型安全的 SSL ex_data 封装
pub(crate) struct SslExData<T: 'static> {
    idx: AtomicI32,
    init: std::sync::Once,
    _marker: std::marker::PhantomData<T>,
}

impl<T: 'static> SslExData<T> {
    pub const fn new() -> Self { /* ... */ }

    pub unsafe fn store(&self, ssl: *mut boring_sys::SSL, value: T) -> Result<(), T> {
        /* 初始化索引 + Box::into_raw + SSL_set_ex_data */
    }

    pub unsafe fn take(&self, ssl: *mut boring_sys::SSL) -> Option<T> {
        /* SSL_get_ex_data + Box::from_raw + 置空槽位 */
    }

    pub unsafe fn peek<R>(&self, ssl: *const boring_sys::SSL, f: impl FnOnce(&T) -> R) -> Option<R> {
        /* SSL_get_ex_data + 闭包访问（不取所有权）*/
    }
}
```

**好处**：
- 消除重复代码（当前两个模块各有约 80 行几乎相同的逻辑）
- 类型安全：`SslExData<SslCtx>` 和 `SslExData<VerifyContext>` 在编译时区分
- `peek` 方法通过闭包限制引用生命周期，防止悬空引用

### 5.3 [P0] 收紧 `insecure_fallback_verify_cb`

1. **添加日志记录**：每次 `insecure_fallback_verify_cb` 被触发时记录连接信息（SNI、来源 IP），以便安全审计。

2. **添加 metric**：暴露一个 `mtls_insecure_fallback_total` 计数器，运维可以对此设置告警。

3. **考虑限制范围**：即使是 fallback 模式，仍应验证证书链的某些属性（例如证书未过期、证书由已知 CA 签发），而非无条件接受。

4. **配置层面**：在 `EdgionGatewayConfig` 的验证逻辑中对 `AllowInsecureFallback` 模式添加 Warning 级别的 event，并在文档中明确声明安全风险。

### 5.4 [P1] 消除 `transmute`，使用更安全的替代

`verify_callback_impl` 中的 `transmute` 可以用显式的指针转换 + 引用创建替代：

```rust
// 替代 std::mem::transmute:
let ssl_ref: &SslRef = &*(ssl_ptr as *const boring_sys::SSL as *const SslRef);
```

虽然底层安全性假设相同，但 `transmute` 是"核武器级别"的 unsafe——它可以在任意类型之间转换，而指针转换链至少让编译器检查指针类型兼容性。此外，`transmute` 在类型大小变化时会产生编译错误而非运行时 UB，这点反而是优势。折中方案是保留 `transmute` 但将其封装在 5.1 建议的集中模块中。

### 5.5 [P1] 增强 `catch_unwind` 的防护

当前 `catch_unwind` 的文档已经列出了局限性。建议补充以下措施：

1. **`verify_callback_impl` 中避免可能 panic 的操作**：审计所有调用路径，确保不使用 `unwrap()`、`expect()`、数组越界访问等。

2. **添加 `#[inline(never)]` 属性**：防止 `verify_callback_impl` 被内联到 `extern "C"` 函数中，确保 `catch_unwind` 的 unwind boundary 有效。

3. **考虑使用 `std::panic::AssertUnwindSafe`**：显式标注 unwind safety，避免编译器在未来版本中更严格地检查 unwind safety 时报错。

### 5.6 [P2] 向上游 (Pingora) 贡献安全 API

长期来看，应该向 Pingora 上游提交 PR，请求：
- 在 `TlsAccept` trait 的回调中提供 ex_data 的安全访问 API
- 提供 `SSL* → &SslRef` 的公开安全转换函数（或至少文档化 `SslRef` 的 ABI 稳定性）
- 在 `handshake_complete_callback` 中通过泛型关联类型传递用户上下文，避免 ex_data

---

## 6. `TlsRef` 与 `SslRef` 的类型混用问题

代码中存在一个容易被忽略的问题：`take_ssl_ctx` 的参数类型是 `&TlsRef`（来自 `pingora_core::protocols::tls`），而 `store_ssl_ctx` 的参数类型是 `&mut SslRef`（来自 `pingora_core::tls::ssl`）。两者都被转换为 `boring_sys::SSL*`。

```rust
// store_ssl_ctx — 使用 SslRef
let ssl_ptr = ssl as *mut SslRef as *mut boring_sys::SSL;

// take_ssl_ctx — 使用 TlsRef
let ssl_ptr = ssl as *const TlsRef as *const boring_sys::SSL;
```

这两个类型的关系取决于 Pingora 的内部实现：
- 如果 `TlsRef` 只是 `SslRef` 的类型别名，则转换是安全的
- 如果 `TlsRef` 是一个包含额外字段的 wrapper，则 `as *const TlsRef as *const boring_sys::SSL` 会**指向错误的内存地址**

**建议**：添加编译时断言验证两者的关系，或统一使用同一类型。

---

## 7. 各 Unsafe 代码块的完整风险矩阵

| # | 位置 | Unsafe 操作 | UB 触发条件 | 概率 | 影响 |
|---|------|-----------|------------|------|------|
| 1 | `tls_pingora.rs:126` | `SslRef → SSL*` | SslRef 布局变化 | 低（依赖升级时） | 致命 |
| 2 | `tls_pingora.rs:169` | `TlsRef → SSL*` | TlsRef ≠ SSL | 中（类型关系不明确） | 致命 |
| 3 | `tls_pingora.rs:109` | `Box::from_raw` free 回调 | 指针已被 take | 极低（代码已处理） | 致命 |
| 4 | `tls_pingora.rs:174` | `Box::from_raw` take | 并发 take（理论不可能） | 极低 | 致命 |
| 5 | `tls_pingora.rs:157` | 原始指针解引用 peek | 并发 take 导致 use-after-free | 极低 | 致命 |
| 6 | `tls_pingora.rs:670` | `insecure_fallback_verify_cb` | 配置错误启用 | 中 | 高（安全绕过） |
| 7 | `mtls_verify_callback.rs:178` | `transmute SSL* → &SslRef` | SslRef 非 transparent | 低（依赖升级时） | 致命 |
| 8 | `mtls_verify_callback.rs:90` | `extern "C"` + `catch_unwind` | 栈溢出 / abort | 极低 | 高（进程崩溃） |
| 9 | `mtls_verify_callback.rs:271` | `SslRef → SSL*` | SslRef 布局变化 | 低（依赖升级时） | 致命 |

---

## 8. 测试建议

### 8.1 单元测试

1. **编译时断言测试**：
   - 在 CI 中为每次 `boring` / `pingora` 依赖升级添加一个检查步骤，验证 `SslRef` 和 `TlsRef` 的 `size_of` 和 `align_of` 与 `boring_sys::SSL` 一致。

2. **ex_data 生命周期测试**：
   - 测试 `store → take` 正常路径：验证数据完整性
   - 测试 `store → SSL 销毁`（不 take）：验证 free 回调被调用，无泄漏（可通过自定义 Drop 计数器检测）
   - 测试 `store 失败` 路径：验证 Box 被回收
   - 测试 `take 空槽位`：验证返回 None

3. **`peek_tls_conn_meta` 安全性测试**：
   - 模拟 `peek` 和 `take` 的交错调用（虽然在正常流程中不应发生）

### 8.2 集成测试

1. **mTLS 握手端到端测试**：
   - 有效客户端证书 → 验证通过
   - 无效 CA 签发的证书 → 验证失败
   - 过期证书 → 验证失败
   - 无证书（`PEER` 模式） → 验证通过（可选 mTLS）
   - 无证书（`FAIL_IF_NO_PEER_CERT` 模式） → 验证失败

2. **SAN/CN 白名单测试**：
   - 证书 SAN 匹配白名单 → 通过
   - 证书 SAN 不匹配 → 拒绝
   - 证书 CN 匹配但 SAN 不匹配 → 拒绝（SAN 优先）

3. **`AllowInsecureFallback` 模式测试**：
   - 验证无证书连接被接受
   - 验证无效证书连接被接受
   - **验证审计日志正确记录了 fallback 行为**

### 8.3 Fuzzing 测试

1. **verify_callback panic 测试**：使用 `libfuzzer` 对 `verify_callback_impl` 的输入进行模糊测试，验证 `catch_unwind` 能捕获所有 panic 路径。

2. **ex_data 并发压力测试**：在高并发连接场景下运行 mTLS 握手，使用 TSan (Thread Sanitizer) 检测潜在的 data race。

### 8.4 Miri 检测

对 ex_data 管理的纯 Rust 部分（`Box::into_raw` / `Box::from_raw` 路径），使用 Miri 工具检测未定义行为：

```bash
MIRIFLAGS="-Zmiri-disable-isolation" cargo +nightly miri test -- ssl_exdata
```

注意：Miri 无法检测 FFI 调用，但可以验证 Box 的所有权转移逻辑。

---

## 9. 总结

当前的 BoringSSL FFI 代码在**功能上是正确的**——在当前的 `boring` / `pingora` 版本下不存在已知的 UB。代码作者也展示了良好的安全意识（`catch_unwind`、null 检查、free 回调、失败时回收 Box）。

然而，这段代码的正确性建立在**脆弱的前提假设**之上：

1. `SslRef` 是 `boring_sys::SSL` 的 `#[repr(transparent)]` wrapper（`foreign_types` 实现细节）
2. `TlsRef` 在内存布局上等价于 `boring_sys::SSL`（pingora 实现细节）
3. BoringSSL 的 free 回调在所有错误路径下都会被调用

**优先级排序**：

| 优先级 | 措施 | 预期工时 |
|--------|------|---------|
| **P0** | 引入 `ssl_ffi.rs` 集中管理指针转换 + 编译时断言 | 1-2 天 |
| **P0** | 统一 `SslExData<T>` 泛型封装 | 2-3 天 |
| **P0** | 收紧 `insecure_fallback_verify_cb` + 添加审计日志 | 1 天 |
| **P1** | 消除 `transmute`，收敛到集中模块 | 0.5 天 |
| **P1** | 增强 `catch_unwind` 防护 | 0.5 天 |
| **P2** | 向 Pingora 上游提交安全 API PR | 持续 |

这些改进不会改变功能行为，但会**显著降低长期维护成本和依赖升级时的 UB 风险**。
