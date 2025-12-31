# Core Plugins 深度审查报告

**日期**: 2025-12-30  
**审查范围**: `core/plugins` 及其子模块 (`edgion_plugins`, `edgion_stream_plugins`, `gapi_filters`)

## 1. 概述 (Overview)

本次审查了以下核心插件模块：
-   **Edgion Plugins** (`core/plugins/edgion_plugins`): `BasicAuth`, `Cors`, `Csrf`, `IpRestriction`, `Mock`.
-   **Stream Plugins** (`core/plugins/edgion_stream_plugins`): `StreamIpRestriction`.
-   **Gateway API Filters** (`core/plugins/gapi_filters`): Header Modifiers, Redirects, Debug, ExtensionRef.

审查主要关注代码质量、性能瓶颈、安全漏洞以及功能完整性。

---

## 2. 严重问题 (Critical Issues) - 需立即修复

### ✅ [已修复] BasicAuth: 严重的性能瓶颈
-   ~~**问题描述**: `BasicAuth` 插件在主插件执行线程中**同步**执行密码哈希验证 (`bcrypt::verify`) 和用户加载 (`bcrypt::hash`)。~~
-   **修复方案**: 
    1.  使用 `tokio::task::spawn_blocking` 将 `bcrypt` 操作移至后台。
    2.  引入 `DashMap` 实现 **TTL 缓存 (5分钟)**，将验证成功的耗时从 100ms+ 降至微秒级。

### ✅ [已修复] BasicAuth: 明文密码 & 兼容性
-   ~~**问题描述**: `load_users` 强制哈希明文，不兼容 Apache `htpasswd` 格式。~~
-   **修复方案**: 
    1.  引入 `htpasswd-verify` 库，支持自适应识别 MD5 (`$apr1$`), SHA1 (`{SHA}`), Bcrypt (`$2a$`) 等格式。
    2.  保留对明文的自动 Bcrypt 升级支持。

### ✅ [已修复] Csrf: 安全漏洞
-   ~~**问题描述**: `CsrfToken` 使用伪随机数 (`rand::random`) 且 Cookie 缺乏安全属性。~~
-   **修复方案**: 
    1.  使用加密安全的 RNG 生成 32 字节 Hex 随机串。
    2.  引入 `cookie` crate，强制开启 `Secure` 和 `SameSite=Lax`。

---

## 3. 功能缺陷与代码改进 (Functional Gaps & Improvements)

### 3.1 错误处理 (Error Handling)
-   **模块**: `gapi_filters` (RequestHeaderModifier, ResponseHeaderModifier, RequestRedirect)
-   **问题**: 许多操作（如设置 Header）的错误被直接忽略 (`let _ = ...`)。
-   **建议**: 至少应记录 Warning 级别的日志，以便排查配置错误或运行时异常。

### 3.2 功能缺失 (Missing Functionality)
-   **ResponseHeaderModifierFilter**:
    -   ✅ **[已修复]** `need remove_response_header` TODO。在 `PluginSession` trait 中添加了接口并已实现删除功能。
-   **RequestRedirectFilter**:
    -   ✅ **[已修复]** 存在 `// TODO: need original matched prefix to do proper replacement` 注释。
    -   ~~**现状**: `replace_prefix_match` 逻辑依赖路由匹配的元数据（匹配了多长），目前上下文中缺失此信息。~~
    -   **修复方案**: 直接从 `PluginSession` 上下文的 `route_unit` 中获取匹配信息 (`matched_info`)，计算前缀长度进行替换。

### 3.3 脆弱的逻辑 (Fragile Logic)
-   **Csrf Plugin**:
    -   ✅ **[已修复]** 手动字符串分割 Cookie 改为使用标准 `cookie` crate 解析。

---

## 4. 插件状态汇总 (Summary Matrix)

| 插件名称 | 状态 | 关键问题摘要 |
| :--- | :--- | :--- |
| **BasicAuth** | � **已修复** | 性能/兼容性/缓存均已优化 |
| **Csrf** | � **已修复** | 真随机数 + Secure Cookie |
| **Cors** | 🟢 良好 | 逻辑基本健全 |
| **IpRestriction** | 🟢 良好 | 基于 `IpRadixTree` 实现 |
| **Mock** | 🟢 良好 | 无阻塞风险 |
| **ResponseHeaderMod** | 🟢 **已修复** | 支持 Remove Header |
| **RequestRedirect** | ✅ **已修复** | 前缀重写支持已实现 |
| **GAPI Filters** | 🟡 警告 | 错误日志被吞没 |

## 5. 后续行动建议 (Action Items)

1.  **P1**: 统一插件的错误处理机制，避免静默失败。
