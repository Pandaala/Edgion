# CPU/Memory 性能审计方法论

> 来源：2026-04 CPU/Memory Review 中误报率过高（49%）的问题复盘。本文提供**通用判定方法**，避免常见误报。
> 具体场景请查阅同目录的 `not-a-performance-issue.md`。

---

## 核心原则

**追溯调用链，区分阶段，验证机制**。

很多看似严重的性能问题，在代码实际执行路径中并不触发。

---

## 必须区分的关键阶段

```
┌─────────────────────────────────────────────────────────────────┐
│  配置加载阶段 (冷路径)          请求处理阶段 (热路径)            │
│  ─────────────────────          ─────────────────────            │
│  • preparse() / compile()       • run_request() / handle()      │
│  • from_yaml() / deserialize    • match_host() / route()        │
│  • 资源创建 / 初始化            • 每个请求都会执行               │
│                                                                  │
│  Clone/Regex 编译在这里 → OK    Clone/Regex 编译在这里 → 问题   │
└─────────────────────────────────────────────────────────────────┘
```

**验证问题是否在热路径**：
1. 找到可疑代码的调用点
2. 追溯调用链到入口函数
3. 判断入口是配置变更触发还是每请求触发

---

## Rust 性能模式检查清单

### Arc 包装后 Clone 是 O(1)

```rust
// ❌ 误报：看起来有 Clone 开销
let config = self.plugin_config.clone();  // 如果是 Arc<T>，这只是引用计数+1

// 检查方法：查看类型定义
pub plugin_config: Arc<PluginRuntime>  // Arc Clone = O(1)
pub plugin_config: PluginRuntime       // 值 Clone = O(n) ← 真问题
```

### preparse 阶段的工作不会在热路径重复

Edgion 的 `EdgionPlugins::preparse()` 负责在配置加载时预编译 Regex、预处理条件等。

```rust
// 常见模式
impl Plugin {
    fn preparse(&mut self) {
        self.compiled_regex = Some(Regex::new(&self.pattern).unwrap());
    }
    
    fn handle_request(&self) {
        // 使用 self.compiled_regex，不会重新编译
    }
}
```

**检查要点**：
- 搜索 `preparse`、`compile`、`compile_conditions` 方法
- 确认 `#[serde(skip)]` 的运行时字段是否在初始化阶段设置

### fallback 代码是防御性编程

```rust
// 这种代码通常不是性能问题
if self.compiled.is_none() {
    // fallback：只在 preparse 失败时触发
    self.compiled = Some(compile_pattern(&self.pattern)?);
}
```

**判断方法**：检查正常流程中 `compiled` 是否已被 `preparse()` 设置。

---

## 内存增长分析原则

### 区分 "与规模成正比" vs "无限增长"

| 场景 | 判断 |
|------|------|
| HashMap 存储 K8s 资源状态 | ✅ 正常 — 与集群规模成正比 |
| 每次请求都往 Vec push | ❌ 泄漏 — 无限增长 |
| 离线节点保留历史数据 | ⚠️ 检查是否有清理机制 |

### 检查已有的清理机制

**必须搜索的关键词**：
- `remove` / `delete` / `clear` / `gc` / `cleanup`
- `ttl` / `timeout` / `expire`
- `max_entries` / `capacity` / `limit`
- Admin API / DELETE endpoint

```rust
// 示例：代码注释说明了设计决策
// Offline entries are retained in memory and must be removed explicitly via
// the Admin DELETE API (see ticket #20).
```

**如果找到清理机制** → 不是泄漏，是设计决策。

---

## 验证流程

```
发现可疑代码
    ↓
追溯调用链 → 找到入口函数
    ↓
是热路径？
    ├─ 否 → ❌ 误报（冷路径）
    └─ 是 → 检查 Arc/Box 包装
                ├─ 有包装 → Clone O(1)？
                │             ├─ 是 → ❌ 误报（Arc Clone）
                │             └─ 否 → ✅ 确认问题
                └─ 无包装 → ✅ 确认问题
```

---

## 常见误报模式

| 看起来像问题 | 实际情况 | 检查方法 |
|-------------|----------|----------|
| `config.clone()` 在处理函数 | config 是 `Arc<T>` | 查看类型定义 |
| `Regex::new()` 在 match 代码 | 有 `preparse()` 预编译 | 搜索 preparse 调用 |
| HashMap 无限增长 | 存储当前状态快照 | 检查是否跟踪外部资源 |
| 离线数据不清理 | 有 Admin DELETE API | 搜索 remove/delete 方法 |
| 大枚举 Clone 开销 | 只在配置加载时 Clone | 追溯 Clone 调用链 |
| 重复 Entry 类型结构体 | 类型安全，不影响运行时 | 确认是编译期还是运行时 |

---

## 报告格式

对每个风险点，验证后的报告必须包含：

```markdown
### [风险点名称]

**原始评级**: 🔴 高
**验证结果**: ❌ 误报 / ✅ 确认 / ⚠️ 潜在

**调用链分析**:
- 入口函数: `xxx`
- 触发条件: 配置变更 / 每请求 / 定时任务

**关键代码**:
[代码片段]

**结论**: [原因说明]
```

---

## Red Flags — 需要重新验证

如果 review 时出现这些想法，停下来重新验证：

- "这个 Clone 肯定很昂贵" → 检查是否 Arc 包装
- "每次请求都会编译 Regex" → 搜索 preparse/compile 方法
- "这个 HashMap 会无限增长" → 检查是否跟踪外部资源状态
- "没有看到清理代码" → 搜索 Admin API / gc / ttl
