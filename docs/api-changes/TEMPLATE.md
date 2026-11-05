# API 变更说明模板

> 复制本文件为 `v<version>.md`（例如 `v0.2.0.md`），按实际变更填写。  
> 规则与流程见 [README.md](./README.md)。

| 项 | 内容 |
|---|---|
| 版本 | vX.Y.Z |
| 日期 | YYYY-MM-DD |
| 兼容级别 | A（Additive）/ B（Behavioral）/ C（Breaking signature）/ D（Removal） |
| 影响 crate | `tslib-core` / `tslib-audio` / ... |
| 影响绑定 | Rust / Java / Python / C |
| 关联 PR/issue | ... |
| 关联 spec | `docs/spec-*.md` 第 X 节 |

---

## 1. 快速迁移清单

> 给下游项目的一页纸。按“必须改 / 建议改 / 无需改”分组。

### 必须改（C/D）

| 旧 API | 新 API | 迁移动作 |
|---|---|---|
| `Client::download_file(cid, path)` | `Client::download_file_with_options(cid, path, opts)` | 传 `FileTransferOptions::default()`；超限会返回 `FileTooLarge` |
| ... | ... | ... |

### 建议改（B）

| 旧行为 | 新行为 | 建议 |
|---|---|---|
| `Connected` 发两次 | 只发一次 | 检查事件计数逻辑 |
| ... | ... | ... |

### 无需改（A）

- 新增 `try_*` 方法；旧方法保留。
- 新增字段 `Channel.max_clients_kind`；旧字段仍可用。

---

## 2. 方法签名变更

| ID | 模块/方法 | 旧签名 | 新签名 | 级别 | 迁移片段 | 绑定影响 |
|---|---|---|---|---|---|---|
| API-001 | `Client::move_to_channel` | `fn move_to_channel(&mut self, cid: u64) -> Result<()>` | `async fn try_move_to_channel(&mut self, cid: u64) -> Result<()>`（旧方法保留 deprecated） | C | `client.try_move_to_channel(cid).await?` | Java `moveToChannel` 增加 password；Python 新增 `try_move_to_channel` |
| API-002 | ... | ... | ... | ... | ... | ... |

### 代码对比

```rust
// 旧
client.move_to_channel(cid)?;

// 新
client.try_move_to_channel(cid).await?;
```

```java
// 旧
client.moveToChannel(cid);

// 新
client.moveToChannel(cid, null); // 或 client.tryMoveToChannel(cid).join()
```

```python
# 旧
client.move_to_channel(cid)

# 新
client.try_move_to_channel(cid)
```

```c
/* 旧 */
tslib_client_move_to_channel(client, cid);

/* 新：保留旧符号；新增带结果的版本 */
tslib_client_try_move_to_channel(client, cid, &err);
```

---

## 3. 返回结构变更

> 字段级对照。没有变更的字段可省略。

| 结构 | 字段 | 旧类型 | 新类型 | 旧含义 | 新含义 | 迁移 |
|---|---|---|---|---|---|---|
| `Channel` | `order` | `i32` | `u64` | 截断的排序值 | 前驱频道 id | 不要按数值排序；用 `ChannelTree` |
| `Channel` | `max_clients_kind` | 无 | `MaxClientsKind` | - | Unlimited/Inherited/Limited/Unknown | 读取新字段 |
| `Event::TextMessage` | `target_id` | 无 | `Option<u64>` | - | 频道/私聊目标 id | 模式匹配补 `..` 或新字段 |
| `ServerInfo` | `uid` | 空串 | 计算值 | 不可用 | 服务器 UID | 可直接使用 |

### 结构示例

```rust
// 旧
pub struct Channel {
    pub order: i32,
    // ...
}

// 新
pub struct Channel {
    pub order: u64,
    pub max_clients_kind: MaxClientsKind,
    // ...
}
```

---

## 4. 类型、字段、枚举变更

| 项 | 旧 | 新 | 级别 | 说明 |
|---|---|---|---|---|
| `Error` | 封闭枚举 | `#[non_exhaustive]` + 新变体 | C | 下游 match 需加 `_` |
| `ConnectionState` | 5 个变体 | 新增 `Disconnecting` | C | 绑定层映射新增整数 |
| `EchoCancellation` | `bool` | 枚举 `Disabled/Speex/Aec3` | C | 旧 `true` 映射为 `Speex`（或 `Aec3`，按迁移表） |

---

## 5. 行为语义变更（签名不变也要写）

| ID | API/事件 | 旧行为 | 新行为 | 影响 | 迁移 |
|---|---|---|---|---|---|
| API-010 | `Connected` | 发两次（第一次 server_name 为空） | 只发一次 | 事件计数 | 删除对重复事件的兼容逻辑 |
| API-011 | `move_to_channel` | 发送即返回 | 等待服务器确认，失败返回错误 | 调用耗时/错误处理 | 改用 `try_*` |
| API-012 | 默认 `initial_input_muted` | 硬编码 true | 配置项，默认 true | 行为不变 | 无需改；常开语音需显式设 false |
| API-013 | 文件下载 | 超过 10 MB 静默截断 | 超限报 `FileTooLarge` | 旧代码可能收到新错误 | 处理新错误或开启流式 |

---

## 6. 绑定层变更

### 6.1 Java / JNI

| 项 | 旧 | 新 | 迁移 |
|---|---|---|---|
| `Client.moveToChannel` | `(long channelId)` | `(long channelId, String password)` + 兼容重载 | 旧调用继续可用 |
| `Client.waitEvents` | 未声明 | 新增 native | 可选使用 |
| `EchoCanceller` | 无 Java 类 | 新增 | 可选使用 |

**JNI 符号**：旧符号保留；新增符号列在这里。

### 6.2 Python

| 项 | 旧 | 新 | 迁移 |
|---|---|---|---|
| `ChatHistory.add_*` | 不存在 | 新增 | 直接调用 |
| `Client.server_info()` | 缓存值 | 实时值 | 无需改，数值更准 |
| `Client.try_move_to_channel` | 不存在 | 新增 | 可选 |

### 6.3 C / FFI

| 项 | 旧 | 新 | 迁移 |
|---|---|---|---|
| 事件驱动 | 无 | `tslib_client_pump` / callback | 在循环中调用 pump |
| ABI 版本 | 无 | `tslib_abi_version()` | 启动时校验 |

---

## 7. 弃用与移除

| 项 | 弃用版本 | `#[deprecated]` note | 计划移除 | 替代 |
|---|---|---|---|---|
| `Client::move_to_channel` | 0.2.0 | use `try_move_to_channel` | 0.4.0 | `try_move_to_channel` |
| `ClientConfig::auto_reconnect` | 0.2.0 | handled by tsclientlib; not honored | 0.4.0 | 无（等待 B 级 supervisor） |

---

## 8. 验证

- [ ] 旧 API 示例仍可编译（至少一个 minor 周期）。
- [ ] 新 API 示例可编译并运行。
- [ ] `cargo-semver-checks` / `cargo public-api` 差异与本文件一致。
- [ ] Java `javap -s` 对比、C header diff、Python stub diff 已检查。
- [ ] 绑定 smoke 测试通过。
- [ ] CHANGELOG 已链接本文件。

---

## 9. 关联

- Spec：`docs/spec-error-fixes-performance.md`
- CHANGELOG：`CHANGELOG.md` vX.Y.Z
- 迁移示例：`examples/migration/vX.Y.Z/`
