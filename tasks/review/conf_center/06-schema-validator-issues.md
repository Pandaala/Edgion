# SchemaValidator：硬编码路径、YAML 分割与校验语义 — 架构评审

**范围**：`src/core/controller/conf_mgr/schema_validator.rs`  
**评审日期**：2026-03-30  
**结论摘要**：非 K8s 模式下本地 OpenAPI schema 校验对「运维可升级性」「多文档 YAML 解析正确性」与「按 kind 的完备校验」存在明显缺口；其中「无 schema 静默通过」在控制面语义上属于**安全/一致性风险**，需与产品策略显式对齐。

---

## 1. 现状与代码锚点

### 1.1 Gateway API 文件路径硬编码

```51:55:Edgion/src/core/controller/conf_mgr/schema_validator.rs
        // Load Gateway API CRDs
        let gateway_api_file = crd_dir.join("gateway-api/gateway-api-standard-v1.5.0.yaml");
        if gateway_api_file.exists() {
            Self::load_gateway_api_crds(&gateway_api_file, &mut validators)?;
        }
```

仓库内同时存在 `gateway-api-standard-v1.5.0.yaml` 与 `gateway-api-experimental-v1.5.0.yaml`，当前实现**仅加载 standard**，experimental CRD 对应的 kind 若被 `ResourceKind` 识别，将无法从该文件获得 schema（与「硬编码单文件」问题叠加）。

### 1.2 多文档 YAML 分割

```138:143:Edgion/src/core/controller/conf_mgr/schema_validator.rs
        // Split by document separator
        for doc in content.split("\n---\n") {
            let doc = doc.trim();
            if doc.is_empty() || doc.starts_with('#') {
                continue;
            }
```

### 1.3 Schema 提取与 `validate` 行为

- `load_single_crd` 与 `load_gateway_api_crds` 内从 `spec.versions[0].schema.openAPIV3Schema` 取 schema 的逻辑重复。
- `validate` 在 `validators` 中无对应 `ResourceKind` 时打 `warn` 并返回 `Ok(())`。

```225:234:Edgion/src/core/controller/conf_mgr/schema_validator.rs
            None => {
                // No schema available for this resource kind
                // Log a warning but don't fail validation
                tracing::warn!(
                    component = "schema_validator",
                    kind = ?kind,
                    "No schema available, skipping validation"
                );
                Ok(())
            }
```

### 1.4 与调用链的关系

- **非 K8s 模式**：`cli/mod.rs` 中 `load_schemas` 在 `from_crd_dir` 失败或 `schema_count() == 0` 时会 **进程退出**，但「至少加载到一个 schema」并不等价于「所有对外暴露的 `ResourceKind` 均有 schema」。
- **K8s 模式**：`validate_resource` 直接跳过本地校验，依赖 API Server；与本文讨论的本地校验路径正交。
- **Admin API**：`validate_resource` 在非 K8s 下调用 `validator.validate`；若某 kind 无 schema，请求仍 **200/成功路径**，仅依赖 warn 日志。

---

## 2. 硬编码路径/版本的影响与迁移方案

### 2.1 具体影响

| 维度 | 说明 |
|------|------|
| **升级摩擦** | Gateway API 发布新版本时，文件名/目录约定变化必须改 Rust 源码并重新发布二进制，无法仅靠替换 `config/crd` 完成升级。 |
| **环境差异** | 不同部署使用不同 Gateway API bundle（版本、standard vs experimental、vendor 裁剪包）时，代码与配置易**漂移**。 |
| **可观测性** | `exists()` 为假时**静默跳过**整份 Gateway API schema，仅减少加载的 kind；若未配套监控 `schema_count` 与期望 kind 列表，问题难以及时发现。 |
| **重复 bundle** | 同目录多文件（standard + experimental）时，当前只读一个文件，易造成「部分 kind 永远无 schema」的隐性状态。 |

### 2.2 迁移方案建议（按侵入性递增）

1. **约定优于配置（小改动）**  
   - 扫描 `crd_dir.join("gateway-api")` 下所有 `*.yaml`（或白名单 glob），依次用同一套「多文档解析 + CRD 提取」加载。  
   - 版本号留在文件名中，由仓库/发布物管理，代码不再写死 `v1.5.0`。

2. **显式配置（中改动）**  
   - 在控制器配置中增加 `schema.gateway_api_globs` 或文件列表，默认指向 `gateway-api/*.yaml`，便于多发行版与 air-gap 环境。

3. **单一 manifest（大改动、长期）**  
   - 由构建脚本生成 `schemas/manifest.json`（kind → schema 路径/内嵌 hash），运行时只读 manifest，彻底解耦「文件命名」与「加载逻辑」。

**推荐优先级**：先实现 **(1) 目录扫描 + 多文件**，再视需要加 **(2) 配置覆盖**。

---

## 3. YAML 文档分割：边界情况分析

Kubernetes / YAML 多文档常用 `---` 作为文档分隔符，但**文本层面**与 `serde_yaml` 的行为并不保证与 `split("\n---\n")` 一致。

### 3.1 当前实现无法可靠处理的合法形态

| 场景 | 示例 / 条件 | 后果 |
|------|----------------|------|
| 首文档无前导换行 | 文件以 `---\napiVersion: ...` 开头 | 第一段包含字面量 `---`，整体 `from_str` 可能失败或解析异常，导致**整段 CRD 被跳过**（Gateway 路径里失败被 `if let Ok` 吃掉）。 |
| Windows 换行 | `\r\n---\r\n` | `split("\n---\n")` **无法分割**，整文件当作单个文档；若单文档过大或结构不符预期，解析行为不稳定。 |
| `---` 带尾随内容 | `--- # end of section`、YAML 1.2 允许的注释 | 分隔符不匹配，**合并或错误切分**。 |
| 行首 `---` 但属块标量内容 | 极少数 YAML 中 `---` 出现在缩进块内 | 简单按行正则也可能误判；需结合「行首 + 整行仅为文档分隔符」的启发式或不用文本分割。 |
| 末尾无换行 | 最后一文档以 `...` 或无结尾换行结束 | `split` 仍能得到最后一段，一般可工作；与 `\r\n` 组合时仍可能出问题。 |
| 以 `#` 开头的「文档」 | 注释块被 `starts_with('#')` 跳过 | 若整段实为「注释 + 后续 CRD」被误合并到上一段，取决于分割是否发生；分割错误时风险更高。 |

### 3.2 更稳健的实现方向

1. **优先使用库能力**：若依赖版本允许，使用支持多文档的 API（例如基于 `serde_yaml` 的文档迭代器模式，或引入 `yaml-rust`/`saphyr` 等先按事件流切 document），避免手写字符串 `split`。  
2. **归一化换行**：读入后统一将 `\r\n` → `\n`，再处理分隔符。  
3. **分隔行判定**：采用行级扫描，匹配「整行（去空白后）为 `---` 或 `---` 后仅空白+注释」作为 document boundary，而不是子串 `"\n---\n"`。  
4. **单文件单 CRD 路径**：`load_single_crd` 已用 `serde_yaml::from_str` 解析整个文件，**不依赖** `---` 分割；Gateway API 若改为「每 CRD 一文件」，可从根本上规避多文档分割问题（以文件数换复杂度）。

---

## 4. 重复 schema 提取逻辑的消除方案

### 4.1 重复范围

两段代码均完成：

- 从 CRD JSON 取 `spec.names.kind`；
- 映射到 `ResourceKind`；
- 从 `spec.versions[*].schema.openAPIV3Schema` 取 schema；
- `Validator::new` 并 `insert`。

### 4.2 重构建议

抽取私有函数，例如：

- `fn try_schema_from_crd_value(crd: &JsonValue) -> Option<(&str /* kind name */, &JsonValue /* schema */)>`  
  或返回 `(ResourceKind, JsonValue)` 在映射成功后。

再由：

- `load_single_crd`：读文件 → 单个 `JsonValue` → 调用抽取函数；
- `load_gateway_api_crds`：按文档切分 → 每个文档 → 调用同一抽取函数。

**好处**：Gateway 与 Edgion CRD 的编译失败日志、字段缺失行为一处维护，减少「一边修了另一边漏修」。

---

## 5. Schema 版本选择策略建议

### 5.1 问题：`versions[0]` 的语义不明

CRD 常含多个 `spec.versions`（例如 `v1alpha2` + `v1beta1` + `v1`）。数组顺序**不应**依赖为「存储版本」或「优先版本」；Kubernetes 以 `spec.versions[].name`、`storage: true` 等元数据表达版本语义。

当前 `.first()` 可能导致：

- 校验使用的是**非存储版本**或旧版本的 OpenAPI schema；
- 用户通过 Admin API 提交的 JSON 形态若对齐「当前主推 API 版本」，可能与所加载 schema **不一致**，出现误报通过或误报失败。

### 5.2 建议策略（可组合）

1. **显式选版**：配置项 `preferred_crd_schema_version: "v1"`，在 `versions` 数组中按 `name` 匹配；不存在则 fallback 到 `storage: true` 的版本。  
2. **存储版本优先**：解析 CRD YAML 中 `version`（单版本 CRD）或 `versions` 里 `storage == true` 的条目（需注意 deprecated 共存）。  
3. **多版本校验（严格）**：对同一 kind 注册多个 `Validator`，以资源 `apiVersion` 选 schema；若无匹配 `apiVersion` 则报错。此方案与 `validate(kind, value)` 仅按 kind 签名的现状相比，需要**传入或从 value 读取 `apiVersion`**，属于 API 扩展。  
4. **文档化当前行为**：若短期维持「单 schema per kind」，至少应在代码注释中写明「取 `versions` 第一个元素，与 K8s 存储版本无关」，避免维护者误判。

**推荐**：短期 **(2) 存储版本或配置优先**；中长期若要与 K8s 行为一致，走向 **(3)**。

---

## 6. 无 schema 时 `validate()` 返回 `Ok` 的安全性讨论

### 6.1 行为本质

在「非 K8s + Admin 写配置」路径下，`ValidationError::NoSchema` 枚举变体**已定义但未使用**；实际行为是 **fail-open**：无 schema → 警告 → 校验通过。

### 6.2 风险

| 风险类型 | 说明 |
|----------|------|
| **完整性** | 错误数据写入配置中心，下游数据面或同步逻辑在更晚阶段才失败，定位成本高。 |
| **安全** | 若 Admin 接口暴露在网络中且依赖 schema 作为**唯一**输入约束，则缺失 schema 等价于对该 kind **关闭校验**；属于纵深防御层面的缺口（需结合认证授权评估）。 |
| **与启动检查的错位** | 启动时只保证 `schema_count > 0`，不保证覆盖所有可写 kind；新加 `ResourceKind` 或新 API 若忘记加入 CRD 包，易出现「线上静默无校验」。 |

### 6.3 策略选项

1. **Fail-closed（推荐用于强一致控制面）**  
   - 无 schema 时返回 `Err(ValidationError::NoSchema)`，`validate_resource` 映射为 `400` 或 `503`（视是否区分「客户端错误」与「服务未就绪」）。  
   - 配套：启动时校验「所有 `ResourceKind` 中参与 Admin 写入的 kind」必须 `has_schema`，否则拒绝启动或仅告警+指标。

2. **可配置策略**  
   - 例如 `schema.validation_mode = strict | warn`：strict 下无 schema 失败；warn 保持现状。默认在 non-k8s 生产环境为 `strict`。

3. **维持 fail-open 但强化可观测性**  
   - 若产品明确接受「无 schema 则仅日志」：应对外文档说明，并对 `kind` 维度打点（counter：`validation_skipped_no_schema_total`），避免仅靠 warn 日志。

---

## 7. 建议落地顺序（供排期）

1. **高**：修正多文档 YAML 解析（或 Gateway bundle 拆文件），避免静默丢 CRD。  
2. **高**：明确「无 schema」策略（fail-closed 或严格可观测 + 文档）。  
3. **中**：Gateway API 目录扫描/配置化路径；加载 experimental（若产品需要）。  
4. **中**：抽取公共 CRD → schema 逻辑；版本选择改为存储版本或配置指定。  
5. **低**：长期 manifest 驱动与按 `apiVersion` 多 schema。

---

## 8. 参考：相关调用与启动约束

- `validate_resource`：`src/core/controller/api/common.rs`（K8s 模式跳过本地校验）。  
- 非 K8s 启动加载：`src/core/controller/cli/mod.rs` 中 `load_schemas`（`from_crd_dir` 失败或零 schema 退出进程）。

---

*本评审基于上述路径当前实现静态分析；若后续分支已修改加载或校验语义，请以最新代码为准。*
