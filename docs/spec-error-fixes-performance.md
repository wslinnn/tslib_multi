# 错误修复与性能优化规格说明

| 项 | 内容 |
|---|---|
| 文档状态 | v1.1（已确认；B1 完成，B2 进行中） |
| 日期 | 2026-11-05 |
| 适用项目 | `tslib_multi` |
| 范围 | `tslib-core`、`tslib-audio`、`tslib-chat`、`tslib-channel`、`tslib-bot`、`tslib-jni`、`tslib-ffi`、`tslib-python` |
| 约束 | **不修改 `tsclientlib/tsproto`、不 fork、不打补丁**；只使用依赖层公开 API |
| 关联文档 | [teamspeak-js-learnings.md](./teamspeak-js-learnings.md) 第 9 节 |
| 性质 | 规格说明；本轮不包含代码实现 |

---

## 1. 概述

### 1.1 问题陈述

当前项目在功能面上已经覆盖连接、语音、文本、文件、频道、Bot 与三种原生绑定，但存在两类问题：

1. **正确性错误**：静默截断文件、Bot/FFI 事件流不推进、Java 绑定签名不匹配、断开后仍可能发送、频道顺序错误、事件丢失、密码频道文件操作失败等。
2. **性能问题**：事件循环轮询延迟、事件/音频深拷贝、说话超时 O(n) 扫描、音频实时回调分配与加锁、解码器只增不减、绑定层阻塞等。

本轮不追求与 `teamspeak-js` 的功能对齐，只解决上述两类问题。

### 1.2 目标

| 目标 | 说明 |
|---|---|
| G1 | 不再出现“静默成功但数据错误”：文件截断、上传未落盘、命令失败不可感知等 |
| G2 | 修复会导致功能不可用或崩溃的错误：Bot、FFI、Java、断开后发送、频道顺序 |
| G3 | 修复状态与服务器不一致：事件缺失、字段丢失、权限提示错误 |
| G4 | 降低事件路径与音频实时路径的延迟、CPU、内存与锁竞争 |
| G5 | 建立可重复的性能基线与回归测试，避免“优化后无数据” |

### 1.3 非目标

- ServerQuery（TCP/SSH/WebQuery）
- TS6 Streams（屏幕共享信令）
- 完整管理命令层（频道/客户端/群组/封禁/权限写）
- 头像/图标全生命周期
- Identity 格式与 `teamspeak-js` 互通
- TS6 capabilities / SHA-256 UID 的完整支持
- 修改 `tsclientlib/tsproto` 的任何行为

### 1.4 成功标准

1. 第 4 节所有 P0 需求验收通过。
2. P1 需求要么验收通过，要么在“依赖层限制”中明确记录并接受。
3. 第 6 节性能指标达到基线目标，且有改前/改后数据。
4. 绑定层 smoke 测试进入 CI。
5. 不引入新的静默失败路径；所有失败返回明确错误或事件。

### 1.5 约束

- **D 层不可改**：`tsclientlib` rev `3fbfa26` 固定；不 fork、不 patch、不 vendored 修改。
- 可使用 D 的公开 API 与已有 feature（如 `unstable`）。
- 需要改 D 才能实现的行为，只能绕行或接受，见第 2.2 节。
- 不引入新的大型依赖，除非有明确收益并经过评审。
- 公开 API 尽量向后兼容；破坏性变更需在本文第 5.4 节列出。

---

## 2. 依赖边界

### 2.1 可用的公开 API

| API | 用途 | 证据 |
|---|---|---|
| `OutCommandExt::send_with_result(con)` | 命令成功/失败关联 | `D/tsclientlib/src/lib.rs:70-71,1621-1626` |
| `StreamItem::MessageResult(MessageHandle, Result)` | 接收命令结果 | `D/tsclientlib/src/lib.rs:230,1366` |
| `ConnectOptions::channel_password/hardware_id/version/input_muted/output_muted` | 配置接线 | `D/tsclientlib/src/lib.rs:1830-1967` |
| `Connection::get_server_key()` | 计算 server UID | `D/tsclientlib/src/lib.rs:839` |
| `AudioData::C2SWhisperNew` | 组 whisper | `D/utils/tsproto-packets/src/packets.rs:220-227` |
| `EccKeyPubP256::to_ts()` | 自算 SHA-256 UID | `D/utils/tsproto-types/src/crypto.rs:218` |
| `data::Server.codec_encryption_mode` | 读取服务器加密模式 | 生成 `structs.rs` |
| `data::Client.optional_data/connection_data/description/country_code` | 补齐用户字段 | 生成 `structs.rs` |
| `data::Channel.order/max_clients/is_unencrypted/...` | 修正频道字段 | 生成 `structs.rs` |
| `Event::PropertyRemoved { extra, invoker }` | 踢人/封禁原因 | `D/utils/ts-bookkeeping/src/events.rs:50-56` |

### 2.2 不改 D 则无法修复的项

| # | 限制 | 影响 | 本规格处理 |
|---|---|---|---|
| L1 | 语音加密固定 `voice_encryption=true` | 无法跟随 `codec_encryption_mode` | 接受；文档记录 |
| L2 | 乱序包提前 ACK，无空洞超时 | 丢包时可能“假活” | B 侧 watchdog 缓解，不根治 |
| L3 | TS6 文件时间按秒解析且不保留原始值 | TS6 文件时间不可用 | 接受；文档记录 |
| L4 | Init0 version / 握手 step 0/2 / puzzle 上限固定 | 极端服务器兼容性受限 | 接受 |
| L5 | 文件 `proto=1` 硬编码 | IPv4-only 环境可能有问题 | 暂接受；实测后评估自研传输 |
| L6 | `capability_extensions` 未进 book | 拿不到 TS6 扩展 | 尝试后若失败则接受 |
| L7 | `clientek` 假加密判定依赖首包 id | 极端重传场景可能失效 | 接受 |
| L8 | `away` 只有 `away_message`，无独立 bool | 只能启发式 | 接受；验证现有逻辑 |
| L9 | `Client` 非 `Send/!Sync` | 不能直接跨线程/`allow_threads` | 同线程或专用线程 + 通道 |

### 2.3 影响

- 本规格中的 P0 需求均不依赖 L1–L9 的修复。
- 若后续产品要求“语音加密跟随服务器”“TS6 文件时间”“TS6 屏幕共享”，需要重新评估是否允许改 D；本轮不做。

---

## 3. 需求总览

> 优先级：P0 = 数据损坏/功能不可用/崩溃；P1 = 状态/事件/绑定缺陷；P2 = 文档/清理。
>
> 批次：B1–B5 对应第 9 节执行顺序。

### 3.1 文件传输

| ID | 需求 | 优先级 | 批次 |
|---|---|---|---|
| FR-FS-01 | 禁止下载静默截断；超限返回明确错误 | P0 | B1 |
| FR-FS-02 | 文件传输使用 idle timeout，而非总超时 | P0 | B1 |
| FR-FS-03 | 提供流式上传/下载；支持 resume/seek | P1 | B4 |
| FR-FS-04 | 文件命令支持频道密码 | P0 | B2 |
| FR-FS-05 | 支持并发文件列表/权限查询，不互相覆盖 | P0 | B2 |
| FR-FS-06 | 上传完成后校验服务器落盘大小 | P1 | B3 |

### 3.2 连接与命令

| ID | 需求 | 优先级 | 批次 |
|---|---|---|---|
| FR-CN-01 | 断开后拒绝新命令；统一连接状态闸门 | P0 | B1 |
| FR-CN-02 | 关键命令返回成功/失败/超时 | P0 | B2 |
| FR-CN-03 | 接线 `channel_password/hardware_id/connect_timeout/version` | P0 | B2 |
| FR-CN-04 | 移动频道只在成功后更新本地状态 | P0 | B2 |
| FR-CN-05 | 重连后恢复订阅/静音/状态 | P0 | B2 |
| FR-CN-06 | 连接期间的非 BookEvents 不丢弃 | P0 | B1 |
| FR-CN-07 | `Connected` 只发一次；状态变更可观测 | P1 | B3 |

### 3.3 事件与状态

| ID | 需求 | 优先级 | 批次 |
|---|---|---|---|
| FR-EV-01 | 补齐 kick/ban/group/ChannelJoined 事件 | P1 | B3 |
| FR-EV-02 | 文本消息携带目标 id | P1 | B3 |
| FR-EV-03 | 映射 D 已提供的 User 可选字段 | P1 | B3 |
| FR-EV-04 | 映射 ServerInfo 的 UID/banner/计数 | P1 | B3 |
| FR-ST-01 | `channel_order` 保持 u64，按前驱链排序 | P0 | B1 |
| FR-ST-02 | `maxclients` 区分 Unlimited/Inherited/Limited | P1 | B3 |
| FR-ST-03 | 权限未知/查询失败不再默认“允许” | P1 | B3 |
| FR-ST-04 | 频道树去重、重挂、环检测、删子树 | P1 | B3 |
| FR-ST-05 | `joinable_channels` 检查人数上限 | P2 | B3 |
| FR-ST-06 | `User.is_talking` 反映真实说话状态 | P1 | B3 |

### 3.4 音频

| ID | 需求 | 优先级 | 批次 |
|---|---|---|---|
| FR-AU-01 | 按 codec 分派解码；非 Opus 不喂 Opus 解码器 | P0 | B2 |
| FR-AU-02 | 校验语音帧大小与 whisper 目标数量 | P0 | B1 |
| FR-AU-03 | 引入按 `seq` 的 jitter buffer | P1 | B4 |
| FR-AU-04 | 按 `seq` gap 触发 PLC | P1 | B4 |
| FR-AU-05 | 设备采样率不匹配时重采样 | P1 | B4 |
| FR-AU-06 | 用户离开/超时清理解码器 | P1 | B4 |
| FR-AU-07 | AEC 接入 AudioManager | P1 | B4 |
| FR-AU-08 | 设备格式协商，减少“打不开设备” | P1 | B4 |

### 3.5 Bot 与绑定

| ID | 需求 | 优先级 | 批次 |
|---|---|---|---|
| FR-BD-01 | Bot 驱动事件流 | P0 | B2 |
| FR-BD-02 | FFI 提供事件泵 | P0 | B2 |
| FR-BD-03 | Java `moveToChannel` 签名与缺失 native | P0 | B2 |
| FR-BD-04 | Python 历史/实时 server info/GIL 说明 | P1 | B3 |
| FR-BD-05 | JNI FileList 用 JSON 库；日志脱敏 | P1 | B3 |

### 3.6 安全与输入

| ID | 需求 | 优先级 | 批次 |
|---|---|---|---|
| FR-SC-01 | BBCode HTML 文本/属性转义与 URL 白名单 | P0 | B1 |
| FR-SC-02 | 密码/私钥/token 不出现在 `Debug`/日志 | P1 | B3 |

### 3.7 性能

| ID | 需求 | 优先级 | 批次 |
|---|---|---|---|
| PR-01 | 事件循环改为事件驱动/批量，降低 p99 延迟 | P1 | B4 |
| PR-02 | 事件与音频负载零深拷贝 | P1 | B4 |
| PR-03 | 说话超时 O(log n)，空闲零扫描 | P1 | B4 |
| PR-04 | `users()`/`channels()` 快照化，`sync_state` 增量 | P1 | B4 |
| PR-05 | 热路径日志降级/限频 | P1 | B4 |
| PR-06 | 事件总线无控制事件丢失；语音 drop-oldest 计数 | P1 | B4 |
| PR-07 | 慢 handler 不阻塞事件泵 | P1 | B4 |
| PR-08 | 采集回调零堆分配、减少锁 | P1 | B4 |
| PR-09 | 播放/mixer 回调零堆分配 | P1 | B4 |
| PR-10 | 音频回调去全局锁或最小化临界区 | P1 | B4 |
| PR-11 | 解码器分片锁/复用缓冲 | P1 | B4 |
| PR-12 | 固定 jitter 60 ms + 播放缓冲 ≤ 40 ms；端到端 p95 ≤ 120 ms | P1 | B4 |
| PR-13 | 丢帧/丢弃事件/队列深度可观测 | P1 | B4 |
| PR-14 | Python 阻塞行为明确；长期专用线程 | P1 | B4 |
| PR-15 | JNI destroy/disconnect 不长时间阻塞 | P1 | B4 |
| PR-16 | 建立 criterion/Android 基准 | P1 | B4 |

### 3.8 文档与迁移

| ID | 需求 | 优先级 | 批次 |
|---|---|---|---|
| FR-DOC-01 | 每次公开 API 变更产出 `docs/api-changes/v<version>.md` | P0 | 每次 |
| FR-DOC-02 | 每个变更记录旧/新签名、返回结构、行为语义、绑定影响、迁移片段 | P0 | 每次 |
| FR-DOC-03 | CI 运行 `cargo-semver-checks` / `cargo public-api`，差异作为文档输入 | P1 | 每次发布的 CI |
| FR-DOC-04 | 弃用项给出 `#[deprecated(since, note)]` 与移除版本 | P1 | 每次 |

---

## 4. 详细需求

### 4.1 文件传输

#### FR-FS-01 禁止静默截断

**现状**

`crates/tslib-core/src/client.rs:565-606` 对下载大小执行 `size.min(10_485_760)`，超过 10 MB 时只读取前 10 MB，随后仍发送 `FileDownloaded`。

**目标行为**

1. 绝不截断。
2. 若文件大小超过配置上限，返回 `Error::FileTooLarge { size, limit }`，不发送 `FileDownloaded`。
3. 若读取字节数不足，返回 `Error::FileTransferCorrupted`。
4. 若服务器在传输中失败，返回 `Error::FileTransferFailed`。

**配置**

```rust
pub struct FileTransferConfig {
    /// 内存下载上限；None 表示不限制（不推荐）。
    pub max_memory_download: Option<u64>, // 默认 Some(256 * 1024 * 1024)
    /// 每次读/写无进展的超时；None 表示不超时。
    pub idle_timeout: Option<Duration>,   // 默认 Some(30s)
    /// 分块大小。
    pub chunk_size: usize,                // 默认 64 KiB
}
```

**接口**

```rust
pub fn set_file_transfer_config(&mut self, config: FileTransferConfig);
pub fn file_transfer_config(&self) -> &FileTransferConfig;
```

**验收**

- 20 MB 文件上传/下载后 SHA-256 一致。
- 超过 `max_memory_download` 的文件返回 `FileTooLarge`，且不产生 `FileDownloaded`。
- 断线/超时返回明确错误，不报成功。

**测试**

- 单元：`FileTransferConfig` 默认值与边界。
- 集成：20 MB 往返；超限；读取中断。

---

#### FR-FS-02 idle timeout

**现状**

- 下载 `read_exact` 总超时 5 s（`client.rs:575`）。
- 上传 `write_all` 总超时 10 s（`client.rs:611`）。

**目标行为**

- 超时按“连续无字节进展”计算，而不是整个传输的总时长。
- 每次成功读到/写入一块就重置计时。
- 超时返回 `Error::FileTransferTimeout { idle: Duration }`，并尝试 `ftstop`（若可用）。

**验收**

- 1 MB/s 的 100 MB 传输不会因总时长超时。
- 服务器停止发送 30 s 后失败。

---

#### FR-FS-03 流式与续传

**目标**

- 下载：`download_file_to_path(cid, remote_path, local_path, options)`，边读边写，内存占用 ≤ `chunk_size`。
- 上传：`upload_file_from_path(cid, remote_path, local_path, options)`，按块读取。
- 续传：
  - 下载 `seek_position: Option<u64>` 传给 `Connection::download_file`。
  - 上传 `resume: bool` 传给 `Connection::upload_file`，并处理返回的 `seek_position`。
- `overwrite` 透传。

**接口**

```rust
pub struct FileTransferOptions {
    pub channel_password: Option<String>,
    pub seek_position: Option<u64>,
    pub resume: bool,
    pub overwrite: bool,
}

pub fn download_file_to_path(
    &mut self, cid: u64, remote_path: &str, local_path: PathBuf, options: FileTransferOptions,
) -> Result<u64>;

pub fn upload_file_from_path(
    &mut self, cid: u64, remote_path: &str, local_path: PathBuf, options: FileTransferOptions,
) -> Result<u64>;
```

**验收**

- 流式下载 1 GB 文件内存不随文件增长。
- 中断后 `seek_position` 续传成功。
- 上传 resume 从服务器返回的偏移继续。

---

#### FR-FS-04 频道密码

**现状**

`client.rs:1390,1422,1433-1435,1447` 固定 `cpw=""`；`download_file/upload_file` 传 `None`。

**目标**

- 所有文件 API 增加 `channel_password: Option<&str>`。
- 控制命令中的 `cpw`/`tcpw` 使用 `tsproto_types::crypto::encode_password` 编码。
- 未提供密码时发送空串（保持现有行为）。

**接口**

```rust
pub fn list_files_with_password(&mut self, cid: u64, path: &str, password: Option<&str>) -> Result<()>;
pub fn delete_file_with_password(&mut self, cid: u64, name: &str, password: Option<&str>) -> Result<()>;
pub fn rename_file_with_password(&mut self, ...) -> Result<()>;
pub fn create_directory_with_password(&mut self, ...) -> Result<()>;
pub fn download_file_with_options(&mut self, cid, path, options: FileTransferOptions) -> Result<()>;
pub fn upload_file_with_options(&mut self, cid, path, data, options: FileTransferOptions) -> Result<()>;
```

保留旧方法作为 `password=None` 的包装。

**验收**

- 密码频道可列目录、上传、下载、删除、重命名、建目录。
- 错误密码返回服务器错误。

---

#### FR-FS-05 并发 pending

**现状**

- `pending_file_list: Option<...>`（`client.rs:105`）单槽。
- `pending_perm_overview: Option<...>`（`client.rs:108`）单槽。

**目标**

- `pending_file_lists: HashMap<FileListKey, PendingFileList>`，键为 `(channel_id, path)` 或请求 id。
- `pending_perm_overviews: HashMap<u64, PendingPermOverview>`，键为 channel id。
- 同一键重复请求：返回 `Error::RequestInFlight`，或覆盖旧请求并让旧调用方超时（二选一，本文选“返回错误”）。

**验收**

- 同时请求两个不同频道的文件列表/权限，均成功。
- 同一频道并发请求返回明确错误。

---

#### FR-FS-06 上传校验

**目标**

- 上传完成（服务器关闭连接）后，若权限允许，发送 `ftgetfileinfo` 校验服务器持有的 size。
- 若 size 不匹配，返回 `Error::FileTransferCorrupted`。
- 若无浏览权限（2568），记录 warning，不视为失败。

**验收**

- 模拟服务器少写字节 → 返回错误。
- 无权限场景 → 成功但 warning。

---

### 4.2 连接与命令

#### FR-CN-01 断开命令闸门

**现状**

`disconnect()` 设置 `state=Disconnected`，但保留 `connection`（`client.rs:1459-1487`）；发送方法只检查 `connection.is_some()`。

**目标**

1. 新增 `ConnectionState::Disconnecting`（或私有 `accept_commands: bool`）。
2. `disconnect()` 立即关闭命令闸门。
3. 所有发送入口调用 `self.ensure_connected()?`：
   - 移动、消息、poke、文件、音频、whisper、权限查询、server variables。
4. 断开后调用返回 `Error::Connection(ConnectionError::NotConnected)`。
5. `connect()` 在 `Disconnecting` 期间等待断开完成。

**验收**

- `disconnect()` 返回后调用任意发送 API 返回 `NotConnected`，且不产生网络包。
- `connect()` 与 `disconnect()` 并发不产生双重连接。

---

#### FR-CN-02 命令结果

**目标**

- 关键命令使用 `OutCommandExt::send_with_result`：
  - `clientmove`、`clientkick`、`clientpoke`、文本消息、文件删除/重命名/建目录、`clientupdate`、`channelsubscribe`/`unsubscribe`。
- `send_with_result` 返回 `MessageHandle`；在 `process_events` 中匹配 `StreamItem::MessageResult`，用 oneshot 唤醒调用方。
- 支持 `command_timeout: Duration`（默认 10 s）。
- 失败返回 `Error::Server { code, message, extra, failed_permission }`。
- 超时返回 `Error::CommandTimeout { command, timeout }`。
- **不承诺**通用 `Vec<Row>`；需要返回数据的命令继续使用现有 typed 消息解析（如 `PermOverview`、`FileList`）。

**接口（新增，避免破坏现有绑定）**

```rust
pub async fn try_move_to_channel(&mut self, cid: u64) -> Result<()>;
pub async fn try_move_to_channel_with_password(&mut self, cid: u64, password: Option<&str>) -> Result<()>;
pub async fn try_kick_client(&mut self, clid: u16, reason: &str) -> Result<()>;
pub async fn try_send_server_message(&mut self, msg: &str) -> Result<()>;
pub async fn try_send_channel_message(&mut self, msg: &str) -> Result<()>;
pub async fn try_send_private_message(&mut self, clid: u16, msg: &str) -> Result<()>;
pub async fn try_delete_file(&mut self, cid: u64, name: &str) -> Result<()>;
```

**兼容策略**

- 保留现有同步方法（`move_to_channel`、`send_server_message` 等）作为 fire-and-forget 兼容入口，标记 `#[deprecated(note = "use try_* for command result")]`。
- 内部调用方（Bot、JNI、Python）改用 `try_*`；绑定层可 `block_on` 或提供异步方法。
- 这样不强制现有 Android/Python/C 调用方立即迁移，但新代码能拿到结果。

**验收**

- 服务器拒绝时返回错误码。
- 超时返回 `CommandTimeout`，不永久挂起。
- 成功命令不产生 `CommandError` 事件。

---

#### FR-CN-03 配置接线

**必须接线**

| 字段 | 接线 |
|---|---|
| `channel_password` | `ConnectOptions::channel_password` |
| `hardware_id` | `ConnectOptions::hardware_id` |
| `connect_timeout` | `wait_connected` 的 deadline |
| `version`（新增） | `ConnectOptions::version` |
| `initial_input_muted`（新增） | `ConnectOptions::input_muted`，或连接后 `set_input_muted` |

**重连字段处理**

`auto_reconnect/reconnect_delay/max_reconnect_attempts` 当前无消费点，且 D 内部仅对 Timeout 重连。本轮：

- 标记 `#[deprecated]`，文档说明“由依赖层处理，B 不保证”。
- 不删除字段（避免绑定破坏）。
- 重连后的状态恢复由 FR-CN-05 保证。

**验收**

- 配置的频道密码可加入密码频道。
- 自定义 hardware_id 出现在服务器端。
- `connect_timeout` 生效（慢服务器按配置失败）。
- `version` 可切换。

---

#### FR-CN-04 移动状态

**问题**

`move_to_channel` 在发送后立即写 `self.channel_id`（`client.rs:1218`）。

**目标**

- 仅在 `send_with_result` 成功或收到自身 `ClientChannel` 变更后更新 `channel_id`。
- 失败时保持原值。

**验收**

- 服务器拒绝移动时 `channel_id()` 不变。

---

#### FR-CN-05 重连恢复

**目标**

在检测到重连完成（`Reconnecting` 后首个 `BookEvents`）：

1. 重新发送 `channelsubscribeall`（若配置为全订阅）。
2. 重新 `sync_state`。
3. 重新发送 `servergetvariables`。
4. 重新应用 `self_input_muted`。
5. 清空 whisper 目标（client/channel id 已失效）。
6. 发送 `Event::Reconnected`（新增变体，避免修改 `Connected`）。

**验收**

- 断线重连后能看到用户/频道。
- 静音状态与重连前一致。
- whisper 不会发往旧 id。

---

#### FR-CN-06 连接期事件缓存

**问题**

`wait_connected` 循环只处理 `BookEvents`，其他 `StreamItem` 被丢弃（`client.rs:384-391`）。

**目标**

- 增加 `pending_events: VecDeque<Event>`。
- 连接期间收到非 BookEvents：正常处理，把生成的事件放入 `pending_events`。
- `wait_connected` 完成后，下一次 `process_events`/`wait_events` 先返回 `pending_events`。
- 音频事件：最多缓存 256 条，超出 drop-oldest 并计数。

**验收**

- 连接期间发送的文本消息在 `Connected` 后可见。
- 音频缓存不超过上限。

---

#### FR-CN-07 Connected 一次

**问题**

`do_connect` 发一次 `Connected`（空 server_name），`wait_connected` 再发一次（`client.rs:186,426`）。

**目标**

- `do_connect` 不再发 `Connected`，只设置 `Connecting`。
- `wait_connected` 发一次 `Connected`。
- `ConnectionStateChanged` 在每次状态切换时发射（复用或替换死模块）。

**验收**

- 一次连接只收到一次 `Connected`。
- 状态序列为 `Connecting -> Connected`。

---

### 4.3 事件与状态

#### FR-EV-01 事件补齐

**目标**

从 D 的 `Event::PropertyRemoved { extra.reason, invoker }` 与 `PropertyChanged` 映射：

| 条件 | 事件 |
|---|---|
| `reason == KickChannel` | `UserKickedFromChannel` |
| `reason == KickServer` | `UserKickedFromServer` |
| `reason == KickServerBan` | `UserBanned` |
| 自身 `ClientChannel` 变化 | `ChannelJoined` |
| `ClientServerGroups` old/new 差集 | `ServerGroupAssigned/Removed` |
| `ClientChannelGroup` 变化 | `ChannelGroupChanged`（新增或复用） |

**验收**

- 每个枚举有至少一个生产者测试。
- 被踢/被封禁的事件带 reason 与 invoker。

---

#### FR-EV-02 文本消息目标 id

**目标**

`Event::TextMessage` 增加 `target_id: Option<u64>`：

- Server：`None`
- Channel：频道 id
- Private：目标 client id

D 的 `MessageTarget::Channel` 不带频道 id；使用 `invoker` 所在频道或目标字段补全。

**兼容性**

破坏性字段新增；0.x 接受，绑定层同步。

**验收**

- 频道消息能拿到频道 id；私聊能拿到目标 id。

---

#### FR-EV-03 用户字段

**目标**

映射 D 已有字段：

| B 字段 | D 来源 |
|---|---|
| `description` | `Client.description` |
| `country` | `Client.country_code` |
| `connected_time` | `Client.connection_data.connected_time` |
| `idle_time` | 由 `last_connected`/`connected_time` 推导，或暂无 |
| `created_at` / `last_connected_at`（新增） | `Client.optional_data.created/last_connected` |
| `server_groups` / `channel_group` | 已有 |
| `is_talking` | 见 FR-ST-06 |

**验收**

- 有权限时字段有值；无权限时为 `None`，不是 0/空串。

---

#### FR-EV-04 ServerInfo 字段

**目标**

- `uid`：用 `Connection::get_server_key().to_ts()` + SHA-1（TS3）/SHA-256（TS6）计算。
- `host_banner_url` / `host_banner_gfx_url`：从 `data::Server` 映射。
- `clients_online` / `channels_online` / `uptime`：从 `optional_data` 映射，保持现有 `servergetvariables` 触发。
- `version` / `platform` / `max_clients` / `icon_id`：已有。
- `capability_extensions`：尝试；若拿不到则 `None` + 文档记录。

**验收**

- ServerInfo 字段与服务器一致。

---

#### FR-ST-01 频道顺序

**现状**

- `Channel.order` 为 `i32`（`state.rs:112`），由 `channel.order.0 as i32` 截断（`client.rs:1635`）。
- `ChannelTree` 按数值升序排序（`tree.rs:195-205`）。

**目标**

- `Channel.order: u64`，语义为“前驱频道 id”，不是排序权重。
- 建树算法：
  1. 按 `parent_id` 分组。
  2. 每组中 `order == 0` 的为第一个候选。
  3. 依次找 `order == 上一个频道 id` 的项。
  4. 处理环/缺失：剩余项按 id 升序追加，并记录 warning。
- 保留 `ChannelTree::from_channels` 的 API。

**验收**

- 与官方客户端顺序一致。
- 大 id（>2^31）不截断。
- 环/缺失不 panic、不死循环。

---

#### FR-ST-02 max clients

**目标**

新增：

```rust
pub enum MaxClientsKind {
    Unlimited,
    Inherited,
    Limited(u16),
    Unknown,
}
```

`Channel` 增加 `max_clients_kind` 与 `max_family_clients_kind`；保留旧 `max_clients: i32` 作为兼容字段（`Unlimited/Inherited` 仍为 -1）。

**验收**

- 三种状态可区分；编辑/UI 不再把 Inherited 当 Unlimited。

---

#### FR-ST-03 权限提示

**目标**

- `compute_file_permission_hints` 返回：

```rust
pub struct PermissionHints {
    pub bits: u64,
    pub known: bool,
}
```

- 权限缺失：视为不允许（除非值为 -1，管理员）。
- 查询失败：`known=false`，`bits=0`；不再默认 `JOIN|BROWSE|DOWNLOAD`。
- `Channel.permission_hints` 保留服务器值；`known` 区分“服务器未给”和“计算失败”。

**验收**

- 无下载权限时不显示下载按钮。
- 查询失败时 UI 能区分“未知”。

---

#### FR-ST-04 频道树完整性

**目标**

- `add_channel`：同一 id 不重复插入 children。
- `update_channel`：reparent 前从旧父节点移除。
- `remove_channel`：删除子树或提升子节点（二选一，本文选删除子树并返回删除列表）。
- 路径/打印：环检测，最大深度限制。

**验收**

- 重复添加/移动/删除的树测试通过。

---

#### FR-ST-05 joinable occupancy

**目标**

`joinable_channels` 排除：

- 有密码的频道（现有）。
- spacer（现有）。
- 成员数 ≥ `max_clients`（Limited 时）。

**验收**

- 满员频道不在结果中。

---

#### FR-ST-06 is_talking

**目标**

- `ts_client_to_user` 时，`is_talking = talking_users.contains_key(&id)`。
- `TalkStatusStart` 时置 true；`TalkStatusStop` 时置 false。
- `users()` 返回的状态与事件一致。

**验收**

- 说话时 `user.is_talking == true`；300 ms 后 false。

---

### 4.4 音频

#### FR-AU-01 codec 分派

**现状**

`AudioManager::process_incoming` 忽略 `codec`，一律 Opus 解码（`manager.rs:137-142`）。

**目标**

- `AudioCodec::OpusVoice | OpusMusic`：Opus 解码。
- `SpeexNarrowband/Wideband/UltraWideband/CeltMono`：暂返回 `AudioError::UnsupportedCodec(codec)`，不送入播放。
- 记录一次 warning（限频），避免刷屏。

**验收**

- 非 Opus 频道不输出噪声；错误可观测。

---

#### FR-AU-02 语音帧与 whisper 校验

**目标**

- `send_audio`：`data.len() + 5 + 8 <= MAX_UDP_PACKET_LENGTH`（以 D 的常量为准）；超限返回 `Error::InvalidArgument`。
- `set_whisper_targets` / `send_audio`：
  - 至少 1 个目标。
  - clients ≤ 32。
  - channels ≤ 255。
  - 超限返回 `Error::InvalidArgument`。
- 不在 B 层截断目标；拒绝发送。

**验收**

- 非法输入返回错误，不产生网络包。
- 合法组 whisper 可发送（若本轮实现）。

---

#### FR-AU-03 jitter buffer（固定延迟）

**设计原则**

即时语音对延迟敏感，**不使用自适应 jitter buffer**：目标延迟固定，不随网络抖动增长；迟到超过窗口的包直接丢弃，而不是等待。这样端到端延迟可预测。

**配置**

```rust
pub struct JitterBufferConfig {
    /// 固定目标延迟，默认 60 ms（3 帧 × 20 ms）。
    pub target_delay: Duration,
    /// 最大缓存帧数，默认 3。
    pub max_frames: usize,
    /// 启动预填充帧数，默认 2（40 ms），避免一开机就 underrun。
    pub prefill_frames: usize,
    /// 连续 PLC 上限，默认 3 帧。
    pub plc_max_frames: usize,
}
```

**行为**

1. 每用户一个 `JitterBuffer`：`BTreeMap<u16, AudioFrame>`，容量 `max_frames`（默认 3 帧 = 60 ms）。
2. 以 `seq` 排序；期望序号从首个包开始。
3. **固定窗口**：只接受落在 `[expected_seq, expected_seq + max_frames)` 内的包。
   - 迟于当前播放位置的包：丢弃，计数 `late_dropped`。
   - 超出窗口的未来包：丢弃，计数 `too_far_dropped`。
4. 缺口处理：
   - 若期望包缺失但后续包已到，最多等待 **1 帧时间（20 ms）**，然后 PLC。
   - 缺失包在等待期内到达则正常播放；超过等待期才到则视为迟到丢弃。
   - 连续缺失超过 `plc_max_frames` 后静音，等待下一个真实包。
5. 缓冲满：丢弃最旧且尚未播放的帧，计数 `overflow_dropped`；不扩大延迟。
6. 重连、长时间静音（> 2 s）后重置，重新预填充。

**明确不做**

- 不根据网络抖动动态增大 `target_delay`。
- 不为了等待迟到包而延长播放。
- 不把 jitter buffer 深度作为拥塞控制手段。

**验收**

- 稳态延迟稳定在 60 ms ± 1 帧，不随抖动增长。
- 乱序在 60 ms 内可恢复；超过 60 ms 的迟到包被丢弃并计数。
- 缓冲不超过 3 帧。
- 5% 丢包下 PLC 生效，延迟不增加。

---

#### FR-AU-04 PLC

**目标**

- 检测 `seq` gap，调用 `OpusDecoder::decode_plc`，最多连续 3 帧。
- 超过 3 帧则静音并等待下一个真实包。
- 统计 PLC 次数。

**验收**

- 5% 丢包下无明显断续；PLC 次数可观测。

---

#### FR-AU-05 重采样

**目标**

- 采集：设备采样率 → 48 kHz（Opus 输入）。
- 播放：48 kHz → 设备采样率。
- 使用 `rubato`；质量/延迟可配置。
- 设备采样率 == 48 kHz 时不启用重采样。

**验收**

- 44.1 kHz 设备可正常采集/播放。
- 音高/时长不变。

---

#### FR-AU-06 解码器清理

**目标**

- `UserLeft` 时调用 `AudioManager::remove_user`。
- 超过 `decoder_idle_timeout`（默认 5 分钟）无音频的用户清理。
- 解码器数量上限（如 512），超出淘汰最久未用。

**验收**

- 长时间多用户会话内存不增长。

---

#### FR-AU-07 AEC 接入

**目标**

- `AudioConfig.echo_cancellation` 改为枚举：

```rust
pub enum EchoCancellation {
    Disabled,
    Speex,
    Aec3,
}
```

- `AudioManager` 在采集链路中调用 AEC：
  - 播放的远端参考帧送入 AEC render。
  - 采集帧经 AEC 处理后编码。
- 帧大小/采样率必须满足引擎要求；不满足时返回 `AudioError::UnsupportedFormat`。
- `echo_cancellation` 默认 `Disabled`（保持现状），避免行为突变。

**验收**

- AEC3 在 48 kHz/20 ms 下 ERLE ≥ 15 dB（现有 echo_spike 标准）。
- 关闭 AEC 时行为与现状一致。

---

#### FR-AU-08 设备格式协商

**目标**

- 打开设备前查询支持的配置。
- 优先 48 kHz/单声道；不支持则选最接近的采样率/声道，并启用重采样。
- 失败时返回包含设备名与支持格式的错误。

**验收**

- 常见 44.1 kHz 设备可用。
- 不支持格式时错误可读。

---

### 4.5 Bot 与绑定

#### FR-BD-01 Bot 事件泵

**目标**

- `Bot::run` 每轮调用 `client.wait_events(50)`（同线程，`Client` 非 Send）。
- 处理事件后再 `try_recv` 回复队列。
- 避免用 `select!` 取消 `wait_events` 导致事件丢失。
- Ctrl+C 通过单独的标志/短 sleep 检查。
- owner-only：从 `client.user(sender_id).uid` 获取发送者 UID。
- 插件加载时注册其 `commands()`。

**验收**

- `!ping` 有回复。
- owner-only 命令对 owner 生效。
- 插件命令可触发。

---

#### FR-BD-02 FFI 事件泵

**目标（最小方案）**

- 新增：

```c
int tslib_client_pump(TsLibClient* client, uint32_t timeout_ms);
const char* tslib_client_pump_json(TsLibClient* client, uint32_t timeout_ms);
void tslib_client_set_event_callback(TsLibClient*, TsLibEventCallback, void* userdata);
```

- `pump` 在调用线程执行 `process_events`；文档要求与 `connect` 同线程。
- 事件回调在 `pump` 内同步调用。
- 返回 JSON 数组（调用方负责 `tslib_string_free`）。

**验收**

- C 示例能连接、pump、收到文本消息。
- 文档明确线程要求。

---

#### FR-BD-03 Java 绑定

**目标**

1. `nativeMoveToChannel(long ptr, long channelId, String password)`。
2. Java `moveToChannel(long channelId, String password)` + 兼容重载 `moveToChannel(long channelId)`。
3. 补齐 6 个 native 声明：
   - `nativeWaitEvents`
   - `nativeSetAudioSink`
   - `nativeSendPoke`
   - `nativeGetNetworkStats`
   - `nativeUpdateServerVariables`
   - `nativeSetWhisperTargets`
4. 补 `EchoCanceller.java`（5 个 native）。
5. 补 Opus 的 4 个 native 声明（PLC/FEC/bitrate/expected loss）。
6. 添加签名一致性测试（`javap`/反射或 CI 脚本）。

**验收**

- Java 调用不再 `UnsatisfiedLinkError`。
- 每个 native 有 Java 声明与 Rust 导出。

---

#### FR-BD-04 Python

**目标**

- `ChatHistory` 增加 `add_server_message/add_channel_message/add_private_message`。
- `Client.server_info()` 调用 Rust `server_info()`，不再是缓存。
- 事件字典补齐 `AudioReceived.data`、`FileDownloaded.data`、`FileListReceived.files`（至少不只有长度）。
- GIL：文档说明 `process_events/wait_connected` 会阻塞；长期改为专用线程（不在本轮强制）。

**验收**

- Python 历史非空；server_info 有在线数/uptime；事件字段可用。

---

#### FR-BD-05 JNI 序列化与日志

**目标**

- `FileListReceived` 用 `serde_json` 序列化，不手拼字符串。
- `JNI_OnLoad` 发布版默认日志级别 `Info`/`Warn`；`Trace` 仅 debug 构建。
- 日志不输出消息正文/密码/token。

**验收**

- 文件名含引号/反斜杠/控制字符可解析。
- 发布版日志无正文。

---

### 4.6 安全与输入

#### FR-SC-01 BBCode HTML 转义

**目标**

- `HtmlRenderer` 对所有文本内容做 HTML 转义（`& < > " '`）。
- 属性值同样转义。
- URL 白名单：`http://`、`https://`；图片同。
- 颜色：`#RGB`/`#RRGGBB` 或已知 CSS 颜色名；其他丢弃。
- 尺寸：数字 1–72；其他丢弃。

**验收**

- `<script>alert(1)</script>` 渲染为文本。
- `javascript:alert(1)` 链接被拒绝。
- 正常 BBCode 不受影响。

---

#### FR-SC-02 敏感信息脱敏

**目标**

- `Identity`、`ClientConfig`、`ConnectOptions` 的 `Debug` 不输出私钥/密码/token。
- 日志中的 `client_key_offset`、`password`、`token` 替换为 `[redacted]`。
- 添加单元测试。

**验收**

- `format!("{:?}", identity)` 不含私钥。
- 日志捕获测试通过。

---

## 5. 数据结构与 API 变更

### 5.1 新增类型

```rust
pub struct FileTransferConfig { ... }
pub struct FileTransferOptions { ... }
pub struct PermissionHints { pub bits: u64, pub known: bool }
pub enum MaxClientsKind { Unlimited, Inherited, Limited(u16), Unknown }
pub enum EchoCancellation { Disabled, Speex, Aec3 }
```

### 5.2 修改类型

| 类型 | 变更 | 兼容性 |
|---|---|---|
| `Channel.order` | `i32` → `u64` | 破坏性；0.x 接受 |
| `Channel` | 新增 `max_clients_kind`/`max_family_clients_kind` | 兼容 |
| `Event::TextMessage` | 新增 `target_id` | 破坏性模式匹配 |
| `Event` | 新增 `Reconnected` | 兼容（加变体仍破坏 exhaustive match） |
| `Error` | 新增 `FileTooLarge`/`FileTransferCorrupted`/`UnsupportedCodec`/`InvalidArgument`/`CommandTimeout`；标记 `#[non_exhaustive]` | 破坏性 |
| `ConnectionState` | 新增 `Disconnecting`；标记 `#[non_exhaustive]` | 破坏性 |
| `ClientConfig` | 新增 `version`、`initial_input_muted`、`file_transfer`；重连字段标记 deprecated | 兼容 |

### 5.3 新增方法

```rust
// 文件
pub fn set_file_transfer_config(&mut self, config: FileTransferConfig);
pub fn download_file_with_options(&mut self, cid, path, options) -> Result<()>;
pub fn upload_file_with_options(&mut self, cid, path, data, options) -> Result<()>;
pub fn download_file_to_path(&mut self, cid, remote, local, options) -> Result<u64>;
pub fn upload_file_from_path(&mut self, cid, remote, local, options) -> Result<u64>;

// 命令
pub async fn execute_with_result(&mut self, cmd: OutCommand, timeout: Duration) -> Result<()>;

// 状态
pub fn permission_hints(&self, cid: u64) -> Option<PermissionHints>;
```

### 5.4 破坏性变更与迁移

| 变更 | 迁移 |
|---|---|
| 新增 `try_*` 命令方法 | 旧同步方法保留并标记 deprecated；新代码用 `try_*`，绑定层逐步迁移 |
| `Channel.order` u64 | 绑定层字段类型同步 |
| `Event::TextMessage.target_id` | 绑定层构造器同步 |
| `ConnectionState::Disconnecting` | 绑定层枚举映射新增 |
| `Error` 新变体 | 绑定层错误映射更新 |

在 0.x 版本内接受破坏性变更，但必须在 CHANGELOG 与迁移文档中列出。

---

## 6. 性能要求与测量

### 6.1 指标

| ID | 指标 | 目标 | 测量 |
|---|---|---|---|
| PR-01 | 控制事件投递 p99 | ≤ 10 ms | 内置计时/基准 |
| PR-01 | `process_events` 单次耗时 | ≤ 16 ms | criterion |
| PR-02 | 每事件堆分配（不含 payload） | ≤ 1 次 | dhat |
| PR-02 | 音频 payload 复制 | 0 次（`Arc`/借用） | 代码审查 + dhat |
| PR-03 | 说话超时扫描 | 空闲 0 次；有说话 O(log n) | 基准 |
| PR-04 | `users()`/`channels()` 1000 用户/200 频道 | ≤ 1 ms，无全量克隆 | criterion |
| PR-04 | `sync_state` 上述规模 | ≤ 20 ms | criterion |
| PR-08 | 采集回调堆分配 | 0 次/帧 | dhat |
| PR-09 | 播放/mixer 回调堆分配 | 0 次/回调 | dhat |
| PR-10 | 音频回调锁等待 | 0 次阻塞（try_lock 或环形） | 代码审查 + perf |
| PR-12 | 端到端延迟（含固定 jitter 60 ms） | p95 ≤ 120 ms | 回环测试 |
| PR-13 | 事件丢弃/语音丢弃 | 可查询计数 | 指标 |
| PR-16 | Android 30 分钟通话 | CPU < 5%，内存增长 < 5 MB，underrun < 1/min | Perfetto |

> 以上为初始目标；第 9 节基线建立后可按实测调整，但不得低于基线。

### 6.2 基线

在 Batch 3 结束前建立 `docs/perf-baseline.md`，记录：

- 硬件/系统/Android 型号。
- 编译配置（debug/release、LTO）。
- 事件路径、音频路径、状态路径的基线数据。
- 每个优化项的改前/改后对比。

### 6.3 测量工具

- Rust：`criterion`、`dhat`/heaptrack、`tokio-console`、`perf`。
- Android：Perfetto、Macrobenchmark、`dumpsys media.audio_flinger`。
- 绑定：Python `timeit` + 线程阻塞测试；JNI 吞吐/延迟。

---

## 7. 测试计划

### 7.1 单元测试

- 文件：配置默认值、超限、字节数不足、密码编码。
- 频道：前驱链排序、环、缺失、重复、删除子树。
- 事件：kick/ban 映射、群组 diff、target_id。
- 权限：缺失/失败/管理员。
- BBCode：XSS、URL 白名单、转义。
- 音频：codec 分派、jitter buffer 顺序、PLC 次数、重采样长度。

### 7.2 集成测试

- 真实 TS3 3.13.8 / TS6 6.0.0-beta13.1：
  - 20 MB 文件往返、续传、密码频道。
  - 断线重连、订阅恢复、静音恢复。
  - 踢人/封禁事件。
  - 频道顺序。
- 故障注入：
  - 丢包 5%/10%。
  - 服务器停止发送。
  - 524/超时。
- 绑定 smoke：
  - C `pump` 收消息。
  - Java `moveToChannel` + 事件。
  - Python 历史/server_info/事件字段。

### 7.3 性能测试

- criterion 基准纳入 CI（阈值告警，不阻塞）。
- Android 真机 30 分钟通话。
- 音频回环延迟/AEC ERLE。
- 1000 用户/200 频道状态同步。

### 7.4 回归清单

见 `teamspeak-js-learnings.md` 第 9.6 节；本规格完成后同步更新。

---

## 8. 实施批次

| 批次 | 内容 | 退出标准 |
|---|---|---|
| B1 | FR-FS-01/02、FR-CN-01/06、FR-ST-01、FR-AU-02、FR-SC-01 | 数据正确性测试通过 |
| B2 | FR-FS-04/05、FR-CN-02/03/04/05、FR-AU-01、FR-BD-01/02/03 | Bot/FFI/Java 可用；密码频道可用 |
| B3 | FR-EV-01~04、FR-ST-02~06、FR-BD-04/05、FR-SC-02 | 事件/字段/绑定测试通过；性能基线建立 |
| B4 | FR-FS-03/06、FR-AU-03~08、PR-01~16 | 流式/续传/音频质量/性能达标 |
| B5 | 回归、文档、限制记录 | 全部验收；限制文档完成 |

**依赖关系**

- FR-CN-02（命令结果）是 FR-CN-04、FR-FS-04/06、FR-BD-01 的前置。
- FR-ST-01 是 FR-ST-04/05 的前置。
- FR-AU-03/04 是 FR-AU-06/07 的前置。
- PR-01/02 是 PR-06/07 的前置。

---

## 9. 风险与开放问题

| # | 风险/问题 | 影响 | 处理 |
|---|---|---|---|
| R1 | D 层限制导致部分需求只能接受 | 功能不完整 | 第 2.2 节记录；不承诺 |
| R2 | 破坏性 API 变更影响 Android App | 升级成本 | 0.x + CHANGELOG + 迁移说明 |
| R3 | 音频设备差异导致重采样/AEC 效果不一 | 体验 | 真机矩阵；默认关闭 AEC |
| R4 | jitter buffer 增加延迟 | 体验 | **固定 60 ms（3 帧），不随网络增长；迟到即丢**；可配置但默认不调大 |
| R5 | 性能目标与基线不符 | 无法验收 | 先建基线再定目标 |
| R6 | `send_with_result` 对 notification 应答命令不适用 | 部分命令仍无结果 | 仅用于 error 应答命令；其余保持现状 |
| R7 | `Client` 非 Send 限制绑定方案 | Python/FFI 复杂 | 同线程/专用线程；不做完整 actor |
| R8 | 初始静音默认值 | 产品行为 | 推荐默认 true（隐私安全），可配置 |

**开放问题（附推荐做法；2026-11-05 已确认，作为 v1.1 冻结）**

| # | 问题 | 推荐 | 理由 |
|---|---|---|---|
| 1 | `max_memory_download` 默认值 | **64 MiB** | 移动端内存安全；大文件默认走流式。`None` 保留但需显式开启 |
| 2 | `initial_input_muted` 默认值 | **true** | 隐私安全默认：应用明确解除静音前不发送麦克风；PTT/常开由上层决定 |
| 3 | `auto_reconnect` 等字段 | **保留一个 minor 周期并 `#[deprecated]`**，文档写明“由 D 处理、B 不保证”；不新增同类死字段 | 避免静默无效配置；直接删除会破坏绑定 |
| 4 | 组 whisper API | **本轮不做**，只做直接 whisper 的校验 | 本轮只修错误/性能；组 whisper 是功能，单独排期 |
| 5 | jitter buffer 目标延迟 | **固定 60 ms（3 帧），迟到即丢，不随网络增长（已确认）** | 即时语音延迟敏感；自适应会不断加延迟 |

> 以上为推荐值；确认后写入 v1.1 并冻结。

---

## 10. 验收清单

- [ ] 所有 P0 需求验收通过。
- [ ] P1 需求通过或记录为“接受限制”。
- [ ] 无静默截断/静默失败。
- [ ] Bot/FFI/Java smoke 通过。
- [ ] 密码频道文件操作通过。
- [ ] 频道顺序与官方客户端一致。
- [ ] BBCode XSS 用例通过。
- [ ] 重连后订阅/静音/状态一致。
- [ ] 性能基线建立；关键指标达标。
- [ ] `cargo clippy --workspace -- -D warnings`、`cargo test --workspace`、Android 构建通过。
- [ ] CHANGELOG/迁移文档/限制文档更新。
- [ ] `docs/api-changes/v<version>.md` 已按模板产出，并与 `cargo public-api`/`cargo-semver-checks` 的差异一致。

---

## 11. API 变更管理与迁移文档

### 11.1 目标

下游项目（Android App、Python/C 调用方）需要平滑升级。每次公开 API 变更都必须有一份**人可读的差异文档**，不能只靠 CHANGELOG 或 commit message。

差异文档要回答四个问题：

1. 哪些方法/字段/返回结构变了？
2. 旧代码怎么改？给可直接粘贴的片段。
3. 行为有没有变（即使签名没变）？
4. Java/Python/C 绑定分别受什么影响？

### 11.2 变更分级

| 级别 | 定义 | 下游动作 | 版本策略 |
|---|---|---|---|
| A（Additive） | 新增方法/字段/枚举变体（枚举已 `#[non_exhaustive]`） | 无需修改 | patch/minor |
| B（Behavioral） | 签名不变，但语义/默认值/事件时序改变 | 建议检查 | minor + 文档 |
| C（Breaking signature） | 参数、返回类型、字段类型、结构布局改变 | 必须修改 | minor（0.x） |
| D（Removal） | 删除公开项 | 必须迁移 | 至少提前一个 minor 标记 deprecated |

### 11.3 文件与流程

| 项 | 约定 |
|---|---|
| 目录 | `docs/api-changes/` |
| 流程说明 | `docs/api-changes/README.md` |
| 模板 | `docs/api-changes/TEMPLATE.md` |
| 每次发布 | `docs/api-changes/v<version>.md`（如 `v0.2.0.md`） |
| 入口 | CHANGELOG 对应版本链接到该文件 |
| 时机 | 代码合并前完成，不能发布后补 |

**流程**

1. 开发前在 spec/issue 标注变更级别。
2. 实现时同步更新 `docs/api-changes/v<version>.md`。
3. CI 生成 API diff 作为校对：
   - Rust：`cargo public-api --diff` 或 `cargo-semver-checks`。
   - Java：`javap -s` 对比旧/新 class。
   - C：`cbindgen` 生成 header 后 diff。
   - Python：`pyo3` stub/`pydoc` 对比。
4. Review 时逐条核对“旧/新/迁移/绑定影响”。
5. 发布前把差异文档链接放进 CHANGELOG 与 release notes。

### 11.4 每条变更必须包含

| 字段 | 说明 |
|---|---|
| ID | `API-001` 递增 |
| 模块 | crate/类/文件 |
| 旧签名/旧结构 | 完整签名或字段列表 |
| 新签名/新结构 | 完整签名或字段列表 |
| 变更级别 | A/B/C/D |
| 迁移片段 | Rust/Java/Python/C 中受影响语言的 before/after |
| 行为变化 | 即使签名不变也要写（超时、默认值、事件顺序、错误语义） |
| 绑定影响 | JNI/FFI/Python 是否需要同步；旧符号是否保留 |
| 弃用/移除 | `since`、计划移除版本 |
| 验证 | 对应测试/示例 |

### 11.5 返回结构变更的写法

返回结构不能只写“字段有变”，要给出字段级表：

| 字段 | 旧类型 | 新类型 | 旧含义 | 新含义 | 迁移 |
|---|---|---|---|---|---|
| `order` | `i32` | `u64` | 截断的排序值 | 前驱频道 id | 直接用；不要按数值排序 |
| `is_talking` | 恒 `false` | 实时值 | 不可用 | 当前是否说话 | 可直接使用 |

### 11.6 行为变化的写法

签名不变但语义变化，必须单独列一节，例如：

- `move_to_channel`：旧“发送即返回”，新“等待服务器确认”；失败返回错误。
- `Connected`：旧发两次，新发一次。
- 默认静音：旧硬编码 true，新可配置；默认仍 true。

### 11.7 最佳实践

1. **优先 additive**：新增 `try_*`、`*_with_options`、`*_v2`，旧方法保留并标记 deprecated。
2. **不要原地改返回类型**：新增方法返回新结构；旧方法保持或用适配层。
3. **枚举先 `#[non_exhaustive]` 再加变体**；否则加变体就是 C 级破坏。
4. **默认值变化也是破坏**：必须按 B 级记录并给迁移说明。
5. **绑定层保留旧符号**：JNI 旧 native 名、C 旧函数、Python 旧方法至少保留一个 minor。
6. **弃用要有期限**：`#[deprecated(since = "0.2.0", note = "use try_move_to_channel; removed in 0.4")]`。
7. **示例即测试**：差异文档里的 before/after 片段要能编译/运行，纳入 smoke test。
8. **机器 diff + 人工语义**：`cargo public-api` 只能发现签名变化，发现不了行为变化。

### 11.8 与 CHANGELOG / spec 的关系

| 文档 | 回答什么 |
|---|---|
| Spec（本文） | 为什么做、做什么、验收标准 |
| `docs/api-changes/v<version>.md` | 旧/新 API 对照、怎么迁移、绑定影响 |
| CHANGELOG | 一句话摘要 + 链接到差异文档 |
| Release notes | 用户视角的重点与升级步骤 |

### 11.9 验收

- [ ] 每个 C/D 级变更都有迁移片段。
- [ ] 每个 B 级变更都在“行为变化”一节。
- [ ] Java/Python/C 影响逐项列出。
- [ ] `cargo-semver-checks`/`cargo public-api` 结果与文档一致。
- [ ] 旧/新示例至少各编译一次。
- [ ] CHANGELOG 链接到差异文档。

---

## 附录 A：需求与文件映射

| 需求 | 主要文件 |
|---|---|
| FR-FS-01/02/03/04/05/06 | `crates/tslib-core/src/client.rs` |
| FR-CN-01/02/03/04/05/06/07 | `crates/tslib-core/src/client.rs`、`config.rs`、`connection.rs` |
| FR-EV-01/02/03/04 | `crates/tslib-core/src/events.rs`、`state.rs`、`client.rs` |
| FR-ST-01/02/03/04/05/06 | `crates/tslib-core/src/state.rs`、`tslib-channel/src/tree.rs`、`manager.rs` |
| FR-AU-01~08 | `crates/tslib-audio/src/*`、`crates/tslib-audio-aec3/src/*` |
| FR-BD-01 | `crates/tslib-bot/src/bot.rs`、`plugin.rs` |
| FR-BD-02 | `crates/tslib-ffi/src/lib.rs` |
| FR-BD-03 | `crates/tslib-jni/src/*`、`java/src/main/java/dev/tslib/*` |
| FR-BD-04 | `crates/tslib-python/src/*` |
| FR-BD-05 | `crates/tslib-jni/src/types.rs`、`lib.rs` |
| FR-SC-01 | `crates/tslib-chat/src/bbcode.rs` |
| FR-SC-02 | `crates/tslib-core/src/identity.rs`、`config.rs` |
| PR-01~16 | 见第 9 节性能表 |

## 附录 B：术语

| 术语 | 含义 |
|---|---|
| D | `tsclientlib/tsproto` 依赖层 |
| B | 当前项目 `tslib_multi` |
| 控制事件 | 文本、进出、频道、命令结果等非语音事件 |
| 语音事件 | `AudioReceived`/语音帧 |
| 静默失败 | 返回成功但实际未完成/数据错误 |
| 命令结果 | 服务器 `error id` 对应的成功/失败 |
| pending | 已发起但未收到结果的请求 |
