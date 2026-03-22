# Rust 编码规范

> 目标：保持代码一致性、可读性和安全性。

## 规则 1：使用 `use` 导入，避免长路径内联

代码中引用类型、函数、常量时，**必须**在文件头部用 `use` 导入，使用处用短路径。

```rust
// ✅ 正确：头部导入，使用处简洁
use crate::types::constants::annotations::edgion;
use crate::types::resources::common::ParentReference;
use crate::types::resources::link_sys::SystemConfig;

let proxy_proto = annotations.get(edgion::PROXY_PROTOCOL);
let config = SystemConfig::Webhook(config);

// ❌ 错误：长路径内联，影响可读性
let proxy_proto = annotations.get(crate::types::constants::annotations::edgion::PROXY_PROTOCOL);
let config = crate::types::resources::link_sys::SystemConfig::Webhook(config);
```

**判定标准**：路径超过 2 层（`crate::a::b::Item`）就应该导入。

**例外**：
- 同名类型消歧义时可用全路径（如 `std::io::Error` vs `crate::Error`）
- 宏内部路径（proc macro 生成代码）

## 规则 2：禁止无上下文的 `unwrap()` / `expect()`

生产代码中禁止裸 `unwrap()`。`expect()` 仅在逻辑上保证不会 panic 时使用，且 **必须** 写清原因。

```rust
// ✅ 正确：expect 说明了为什么不会 panic
let tail = self.tail.expect("tail must exist when len > 0");

// ✅ 正确：用 ? 或 match 处理错误
let value = map.get(&key).ok_or_else(|| Error::NotFound(key))?;

// ❌ 错误：裸 unwrap，crash 时无上下文
let value = map.get(&key).unwrap();

// ❌ 错误：expect 没有解释原因
let value = map.get(&key).expect("failed");
```

**例外**：
- 测试代码（`#[cfg(test)]`）中可以自由使用 `unwrap()`
- 静态保证的场景（如 `Regex::new(r"literal")` 编译期确定合法的正则）

## 规则 3：`Clone` 使用须审慎

避免不必要的 `.clone()`，尤其是大结构体。优先使用引用或 `Arc`。

```rust
// ✅ 正确：Arc 的 clone 是 cheap 操作
let store = self.store.clone(); // Arc<RwLock<T>>

// ✅ 正确：需要 owned 值时才 clone
let name = resource.name().to_string();

// ❌ 可疑：clone 一个大结构体只为读取
let route = big_route_config.clone();
let name = route.name; // 只需要 name，不需要 clone 整个结构体
```

## 规则 4：错误信息要有可操作性

错误信息应包含：**什么失败了** + **上下文** + **可能的解决方式**（如适用）。

```rust
// ✅ 正确：包含上下文和建议
format!(
    "Cannot create work directory {}: {}\nPlease check permissions or specify a different directory with --work-dir",
    path.display(), e
)

// ❌ 错误：无上下文
format!("IO error: {}", e)
```

## 检查清单

在 code review 时使用：

- [ ] 是否有超过 2 层的 `crate::` 路径内联在代码中？应提取为 `use` 导入
- [ ] 是否有裸 `unwrap()`？应改为 `?` / `expect("reason")` / `match`
- [ ] `expect()` 消息是否解释了为什么不会 panic？
- [ ] 是否有不必要的 `.clone()`？能否用引用或 `Arc` 替代？
- [ ] 错误信息是否包含足够上下文用于排障？
