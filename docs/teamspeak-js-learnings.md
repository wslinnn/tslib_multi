# 从 teamspeak-js 借鉴什么：现状、改进点与落地路线

> 分析对象：`D:\develop\project\teamspeak-js`（第三方独立实现 `@honeybbq/teamspeak`，纯 TypeScript，下称 **A**）
>
> 当前项目：`D:\develop\project\TS6_Droid_CN\tslib_multi`（Rust + tsclientlib/tsproto，下称 **B**）
>
> 依赖层：`tsclientlib` rev `3fbfa26`（B 的实际协议实现，下称 **D**）
>
> 本文性质：**改进建议，不是已完成的改动**。本轮只读分析，未修改任何代码。
>
> 关联结论：功能矩阵与逐项差异见上一轮对比；本文只保留“值得学什么、怎么落地、先做什么”。
>
> **2026-11-05 方向修订**：当前只关注两件事——**修复错误**与**优化性能**；**不改 tsclientlib、不 fork、不打补丁**。功能对齐（ServerQuery、Streams、管理命令、头像/图标等）暂缓。**当前执行计划看第 9 节**；第 8 节是完整评审背景，其中涉及改动依赖层的建议已按当前约束改写。

---

## 0. 一句话结论

A 最值得借鉴的不是某个功能，而是四件工程化做法：

1. **把协议知识变成单一事实源**：命令名、通知名、属性名、权限、错误码、客户端版本都可枚举、可生成、可校验，而不是散落的 magic number。
2. **每个命令都有明确的请求—应答与错误语义**：`await` 到结果，服务器拒绝抛带错误码/失败权限的异常，超时和取消是一等公民。
3. **事件与状态层面向消费者设计**：typed 事件 + raw notification 兜底 + 每消费者独立缓冲 + 完整 server view + 顺序保证。
4. **把重连、限流、超时、文件传输当作可配置的运行时策略**，而不是硬编码常量。

B 的优势在音频处理链与原生绑定，不应为了“对齐 A”而放弃；正确做法是**把 A 的协议/API 工程化方法移植到 Rust 层**，同时保留 B 在音频、聊天、Bot、Android 上的差异化。

---

## 1. 十三个可借鉴点

### 1.1 命令名 / 通知名 / 属性名 / 权限 / 错误码的单一事实源

**A 的做法**

| 表 | 数量 | 位置 |
|---|---:|---|
| `CommandName` | 172 | `A/src/core/command-name.ts` |
| `NotificationName` | 79 | `A/src/core/notification-name.ts` |
| `PropertyName` | 187 | `A/src/core/property-name.ts` |
| `PermissionName` | 396 | `A/src/core/permission-name.ts` |
| `ErrorCode` | 203 | `A/src/core/errorcode.ts` |
| 客户端版本 | 1536 | `A/src/versions/index.ts` |

这些表都带 TSDoc 注释，很多还注明“实测于 TS3 3.13.8 / TS6 6.0.0-beta13.1”。

**B 的现状**

- 权限 id 直接硬编码在 `crates/tslib-core/src/client.rs:1669-1681`（235–246），注释写“from Permissions.csv, 0-indexed from line 2”。
- 命令名以字符串字面量散落在 `client.rs`（`"clientmove"`、`"ftgetfilelist"`、`"permoverview"` 等）。
- 没有通知名/属性名/错误码表；错误处理只识别极少数错误码（如 1281）。

**建议**

1. 在 `tslib-core` 新增 `protocol` 模块：
   - `protocol/command_names.rs`
   - `protocol/notification_names.rs`
   - `protocol/property_names.rs`
   - `protocol/permission_ids.rs`
   - `protocol/error_codes.rs`
2. 用 `build.rs` 从 tsdeclarations 的 CSV/TOML 生成，或在 workspace 维护一份 `protocol/*.toml` + 生成脚本；生成物入库，CI 校验“重新生成无 diff”。
3. 先做最小闭环：权限 id、文件传输命令名、错误码；再扩展属性名。
4. 禁止新增裸数字/裸字符串；`clippy` 或 code review 拦截。

**验收**：全仓库不再出现权限 magic number；生成常量与 tsproto 声明逐项一致。

---

### 1.2 类型化 ID 与范围校验

**A 的做法**

`ClientID`、`PermissionID`、`IconID` 是 `number`，`ChannelID`、`ClientDBID`、`ServerGroupID` 等是 `bigint`；构造器 `asClientID(5)` 等对范围做校验，越界抛 `RangeError`（`A/src/core/command/ids.ts`）。类型品牌（branded type）让 `ChannelID` 不能传给需要 `ClientID` 的位置。

**B 的现状**

- 公开 API 大量使用 `u16`/`u64`/`i64` 裸类型：`move_to_channel(channel_id: u64)`、`send_private_message(user_id: u16)`、`icon_id: i64`。
- `channel_id` 与 `client_id` 在调用点容易混淆，编译器无法拦截。
- `PermissionId` 直接是 `u32`。

**建议**

```rust
pub struct ClientId(pub u16);
pub struct ChannelId(pub u64);
pub struct DatabaseId(pub u64);
pub struct PermissionId(pub u32);
pub struct IconId(pub u32);

impl TryFrom<u64> for ChannelId { /* 校验非 0、范围 */ }
impl TryFrom<i64> for IconId { /* 兼容有符号 wire 形式 */ }
```

- 公开 API 逐步改为 newtype；保留 `From<u64>` 便捷转换。
- 内部可直接复用 tsproto 的 `ClientId`/`ChannelId`，但不要把它们泄漏到绑定层。
- 对“0 表示无/未知”的语义写进类型文档或 `Option<ClientId>`。

**验收**：把 `ChannelId` 传给 client 参数无法编译；越界构造有单测。

---

### 1.3 缺失值三态与“只发送已设置字段”

**A 的做法**

- 可选字段缺失是 `undefined`，与 `false`/`0`/`""` 严格区分（`A/src/core/api/command.ts:350-396`）。
- `ClientChanges` 只在服务器确实发送某字段时才带该键；`"isAway" in changes` 是可靠判断（`A/src/core/api/clients.ts:176-207`）。
- 编辑命令只发送调用方设置的字段，未设置保持服务器原值；`""` 表示清空（`A/docs/guide.md:216-226`）。

**B 的现状**

- `User`、`Channel`、`ServerInfo` 全是 `Default` 的 `false`/`0`/空串（`crates/tslib-core/src/state.rs:71-224`）。
- 无法区分“服务器未告知”和“服务器说是 false/0”。
- 频道编辑命令尚未实现，但一旦实现，容易把默认值整包发回去。

**建议**

1. 快照类型中，服务器可能不返回的字段改为 `Option<T>`；纯本地派生字段保持 `bool`/`u64`。
2. 编辑参数使用显式三态：

```rust
pub enum FieldUpdate<T> {
    Unset,          // 不发送，保持服务器原值
    Clear,          // 发送空值，清空
    Set(T),
}
```

或 `Option<Option<T>>`，但前者可读性更好。
3. 为每个快照保留 `raw: HashMap<String, String>`，未建模字段不丢，方便向前兼容。
4. 事件里的 `changes` 只包含实际变化的字段，不要整包重发。

**验收**：能写出“未携带 vs false”的单测；编辑命令的序列化测试只包含设置的键。

---

### 1.4 请求—应答关联与统一错误模型

**A 的做法**

- `CommandTracker` 用客户端追加的 `return_code` 把命令与应答配对；服务器用 notification 应答的命令（`servergrouplist`、`ftinitupload`、`setupstream`）有显式响应表（`A/src/client/commands.ts`、`A/src/core/command/responses.ts`）。
- 服务器拒绝抛 `ServerError`，带 `code`、`serverMessage`、`extraMessage`、`failedPermissionID`、`failedPermission`（`A/src/core/errors.ts:74-130`）。
- 命令支持 `signal` 与 `timeoutMs`；超时抛 `CommandTimeoutError`。
- 命令成功但缺少应有数据时抛 `NoDataError`，而不是返回 0/空对象。

**B 的现状**

- 除文件传输初始化外，移动、消息、poke、文件删除/改名/建目录、`clientupdate` 全部 `.send()` fire-and-forget（`crates/tslib-core/src/client.rs:1199-1219,1233-1299,1418-1451,1515`）。
- 调用方无法知道命令是否成功；错误只变成无关联的 `Event::CommandError`。
- `Error` 枚举只有 `Server { code, message }`，没有失败权限、额外信息、NoData 等区分（`crates/tslib-core/src/error.rs:10-46`）。

**建议**

1. 增加统一入口：

```rust
pub async fn execute(&mut self, cmd: OutCommand) -> Result<Vec<Row>>;
pub async fn execute_no_wait(&mut self, cmd: OutCommand) -> Result<()>;
```

内部使用 tsproto 的 `send_command_with_result` / `MessageHandle`，不要自己维护 `return_code`。
2. 高频命令改为返回 `Result`：

```rust
pub async fn move_to_channel(&mut self, cid: ChannelId, password: Option<&str>) -> Result<()>;
pub async fn kick_client(&mut self, clid: ClientId, reason: Reason) -> Result<()>;
pub async fn send_server_message(&mut self, msg: &str) -> Result<()>;
pub async fn delete_file(&mut self, cid: ChannelId, name: &str) -> Result<()>;
```

3. 错误模型补齐：

```rust
pub enum Error {
    Server(ServerError),        // code / message / extra / failed_permid
    NoData(&'static str),       // 命令成功但缺少数据
    Timeout { command: String },
    NotConnected,
    Removed { reason: String }, // kick/ban/server stop
    // ...
}
```

4. notification 应答的命令单独建表（文件传输、服务器组列表等），不要用“收到任意 notification 就当结果”。
5. 保留 `CommandError` 事件，用于“非请求触发的服务器错误”和兼容旧调用方。

**验收**：移动/踢人/发消息/删文件在失败时返回明确错误；集成测试断言错误码与失败权限。

---

### 1.5 传输 / 重连 / 限流参数化

**A 的做法**

| 机制 | 默认值 | 位置 |
|---|---|---|
| 命令超时 | 10 s，可覆盖 | `A/src/client/client.ts:204,945-960` |
| 连接尝试上限 | 30 s/次 | `A/src/client/client.ts:214,2228-2232` |
| 重连退避 | 1 s 起、×2、60 s 封顶、±20% jitter、默认不放弃 | `A/src/core/reconnect.ts:35-42` |
| 重连判定 | 按错误类型，`shouldRetry` 可注入 | `A/src/client/reconnect.ts:16-42` |
| 请求限流 | 令牌桶 4/s、容量 8、FIFO | `A/src/client/throttle.ts:15-22` |
| 防洪 524 | 解析 `retry in Nms` / `please wait Ns`，×2 等待，最多重试 3 次 | `A/src/core/errors.ts:260-306` |

**B 的现状**

- `ClientConfig` 有 `auto_reconnect`、`reconnect_delay`、`max_reconnect_attempts`、`connect_timeout`，但 core 中**没有任何消费点**（`crates/tslib-core/src/config.rs:19-38`；`grep` 仅 Bot 用自己的配置）。
- 依赖层只在 `Timeout` 类错误时固定 `sleep(10s)` 重连（`D/tsclientlib/src/lib.rs:438-448,1198-1212`）。
- 没有命令节流；没有 524 解析与重试。
- 文件/连接超时是硬编码 5 s/10 s/15 s。

**建议**

1. 用配置结构显式建模：

```rust
pub struct ReconnectPolicy {
    pub enabled: bool,
    pub initial: Duration,
    pub max: Duration,
    pub jitter: f32,
    pub max_attempts: Option<u32>,
    pub should_retry: fn(&Error, u32) -> bool,
}

pub struct ThrottleConfig {
    pub rate_per_sec: u32,   // 默认 4
    pub burst: u32,          // 默认 8
}

pub struct TimeoutConfig {
    pub connect: Duration,
    pub command: Duration,
    pub file_idle: Duration,
}
```

2. 在 B 层实现 supervisor：捕获 `StreamItem::DisconnectedTemporarily` 与连接错误，按策略重建 `TsConnection`；不要只依赖依赖层。
3. 发送命令前过令牌桶；解析 524 的 `extra_message`，等待后重试。
4. 重连成功后重放：`channelsubscribeall`、`sync_state`、`set_input_muted`、必要的 server variables。
5. 断开事件携带 `will_reconnect`，不要静默恢复。

**验收**：断网/超时/524 的故障注入测试；配置关闭重连后不再自动连。

---

### 1.6 事件模型：typed + raw + 每消费者缓冲

**A 的做法**

- 28 个 typed 事件全部有生产者；另外所有服务器 notification 都投递给 `notification` 兜底事件，typed 先、兜底后（`A/src/client/client.ts:1758-1764`）。
- `events(name)` 返回异步迭代器，每个消费者独立缓冲；语音缓冲 256，满时丢最旧并计数 `iterator.dropped`（`A/src/client/client.ts:1097-1188,1125-1130`）。
- 事件按到达顺序串行投递；单个监听器抛错不影响其他监听器。
- `view` 在事件之前更新，监听器看到的 view 至少与事件一样新（`A/docs/design.md:161-166`）。

**B 的现状**

- 使用 `tokio::broadcast` 容量 256（`crates/tslib-core/src/client.rs:127`）。慢消费者会丢失事件，且没有“丢了什么/丢了多少”的反馈。
- 没有 raw notification 事件；`process_message_event` 只处理 FileList/FileListFinished/PermOverview/CommandError，其余丢弃（`client.rs:901-1015`）。
- 28 个事件枚举中，`ChannelJoined`、`UserKickedFromChannel`、`UserKickedFromServer`、`UserBanned`、`ServerGroupAssigned`、`ServerGroupRemoved`、`ConnectionStateChanged` 从未由 `Client` 发射。
- `Connected` 发两次（`client.rs:186` 与 `426`），第一次 server_name 为空。

**建议**

1. 保留 `broadcast` 作为兼容层，新增每消费者 `EventStream`：

```rust
pub struct EventStream {
    rx: mpsc::Receiver<Event>,
    dropped: Arc<AtomicU64>,
}
impl EventStream {
    pub fn dropped(&self) -> u64;
}
```

普通事件用有界 mpsc（如 1024）；语音单独通道，满时 drop-oldest，并计数。
2. 增加：

```rust
Event::Notification {
    name: String,
    params: HashMap<String, String>,
}
```

所有未建模 notification 都投递，保证新服务器特性不丢。
3. 补齐事件生产者：
   - kick/ban：从 `notifyclientleftview` 的 `reasonid/reasonmsg` 区分频道踢、服务器踢、封禁。
   - 群组变更：从 `notifyclientupdated` / `notifyclientchannelgroupchanged` 提取。
   - `ChannelJoined`：在收到 `ClientChannel` 变化且目标是自己时发射，或在 `clientmove` 成功应答后发射。
4. 去重 `Connected`：只在 `wait_connected` 后发一次，或第一次发“Connecting”、第二次发“Connected”。
5. 定义顺序契约并写测试：先更新 state，再投递 typed，再投递 notification。

**验收**：慢消费者不阻塞 pump；`dropped` 可观测；每个枚举都有至少一个生产者测试。

---

### 1.7 完整 server view 与 revision

**A 的做法**

- `client.view` 是完整服务器视图：频道树、每频道成员、客户端记录、自身状态、组列表；`ViewStatus` 为 `empty/loading/live/stale`（`A/src/client/view.ts`）。
- `viewReady` 在欢迎序列完成（`channellistfinished`）后触发；`viewChanged` 携带 `revision`；事件先于 view 变更投递（`A/docs/design.md:161-166`）。
- 频道顺序由 `ChannelTree` 根据 `channel_order`（前驱 id）推导，不按数值排序（`A/src/core/channel-tree.ts`）。

**B 的现状**

- `ServerState` 是 users/channels 的 `HashMap` 镜像，查询辅助有限（`crates/tslib-core/src/state.rs:10-66`）。
- `Channel.order` 映射为 `i32`（`client.rs:1635`），`ChannelTree` 按数值升序排序（`crates/tslib-channel/src/tree.rs:195-205`），与 TS3 的前驱链语义不符。
- 没有 revision、没有 viewReady/viewChanged；`wait_connected` 用 15 s 硬超时等第一个 `BookEvents`（`client.rs:371-444`）。

**建议**

1. `ServerView` 作为唯一事实源：`channels`（树 + 索引）、`clients`、`own_client`、`groups`、`revision`、`status`。
2. `channel_order` 保持 `u64`；建树时按“每个频道记录其后继”还原顺序；补环检测。
3. 增加 `view_ready: watch::Receiver<bool>` 与 `view_changed: watch::Receiver<u64>`。
4. `sync_state` 改为“全量快照 + 差异事件”，避免清空重建导致 UI 闪烁。
5. 保留 `raw` 字段，未建模的服务器属性不丢。

**验收**：频道顺序与官方客户端一致；树无环；revision 单调递增。

---

### 1.8 文件传输：流式、续传、停止、校验

**A 的做法**

- 控制面 `ftinitupload`/`ftinitdownload`，数据面独立 TCP；先写 `fileTransferKey` 再传字节（`A/src/client/transfer.ts`）。
- 上传支持 `resume`（服务器返回 `seekPosition`），下载支持 `seekPosition`。
- 下载可写 `Writable` 流式；内存模式上限 256 MiB，超过先 `ftstop` 再报错。
- 空闲 30 s 超时（可配 `0` 关闭）；上传/下载都要求字节数精确匹配。
- 上传结束后用 `ftgetfileinfo` 校验服务器实际持有大小；无浏览权限时退化为“连接关闭即成功”。
- `ftstop` 有严格时间窗：只在拨号失败且距协商 <5 s 时发送，避免 TS6 崩溃（`A/src/client/api/upload-checks.ts:12-45`）。

**B 的现状**

- 下载 `size.min(10_485_760)` 后 `read_exact`，5 s 超时；超过 10 MB **静默截断仍报成功**（`crates/tslib-core/src/client.rs:565-606`）。
- 上传整份 `Vec<u8>` 常驻内存，`write_all` 10 s 超时（`client.rs:607-641`）。
- `download_file`/`upload_file` 的 `seek_position`/`resume` 固定 `None`/`false`；没有 `ftstop`/`ftgetfileinfo`/`ftlist`（`client.rs:1356-1383`）。
- 文件命令的 `cpw`/`tcpw` 固定空串，加密频道文件操作必失败（`client.rs:1390,1422,1433-1435,1447`）。

**建议**

1. 移除 10 MB 截断。两条路：
   - 短期：可配置 `max_download_size`，超限返回 `Error::FileTooLarge`，绝不截断。
   - 目标：流式 API，边读边写文件/回调，内存占用恒定。
2. 增加配置：

```rust
pub struct FileTransferConfig {
    pub max_memory: Option<u64>,     // None = 流式
    pub idle_timeout: Option<Duration>, // None = 不超时
    pub chunk_size: usize,           // 默认 64 KiB
    pub proto: u8,                   // 0/1，默认待实测确定
}
```

3. 透传 `resume`/`seek_position` 与 `channel_password`；依赖层已支持（`D/tsclientlib/src/lib.rs:1645,1669`）。
4. 增加 `ftstop`、`ftgetfileinfo`、`ftlist` 包装。
5. 上传完成做大小校验；校验失败报 `Error::FileTransferCorrupted`。
6. 失败清理遵循 A 的 5 s 时间窗规则，避免 TS6 `ftstop` 崩溃。

**验收**：>10 MB 文件往返一致；断点续传；加密频道可选密码；空闲超时可触发。

---

### 1.9 时间与单位集中处理

**A 的做法**

- `date()` 按数量级阈值 `1e11/1e14/1e17` 自动识别秒/毫秒/微秒/纳秒；0 或缺失返回 `undefined`，不是 1970（`A/src/core/api/command.ts:398-422`）。
- 持续时间统一为毫秒，字段名带 `Ms`；服务器以秒给出的显式换算。
- 语音的 `receivedAt` 是 `performance.now()` 单调时钟，不是 Unix 时间。

**B 的现状**

- `FileEntry.datetime` 直接 `unix_timestamp()`，按秒解析（`crates/tslib-core/src/client.rs:33,913`）。
- TS6 的 `ftgetfilelist` 是毫秒、`ftgetfileinfo` 是纳秒，B 会解析成天文数字或失败。
- 各超时字段单位不统一（`Duration` 与 `u64 ms` 混用）。

**建议**

1. 新增 `protocol/time.rs`：

```rust
pub fn parse_timestamp(raw: &str) -> Option<OffsetDateTime>;
pub fn parse_duration_ms(raw: &str, unit: DurationUnit) -> Option<u64>;
```

2. 所有对外时间字段改为 `Option<OffsetDateTime>` 或 `Option<i64>`（毫秒），并在文档里写单位。
3. 对 TS3/TS6 分别写表驱动测试，包含 0、负数、秒/毫秒/微秒/纳秒样本。

**验收**：TS6 文件列表时间正确；0 显示为“未知”而不是 1970。

---

### 1.10 身份与 TS6 能力

**A 的做法**

- 导出 `base64(32B 私钥标量):offset`，导入严格校验；私钥只通过 `export()` 暴露，`toString/toJSON/inspect` 不泄漏（`A/src/client/crypto/identity.ts`）。
- 安全等级 0–160；`upgradeToLevel` 返回新对象；每 10k 次让出事件循环并支持 `signal`。
- UID 按服务器版本选 SHA-1/SHA-256（`identity.ts:250-286`）。
- `serverCapabilities` 解析 `virtualserver_capability_extensions`、family、uidAlgorithm、server UID（`A/src/client/capabilities.ts:29-95`）。
- 1536 条带 Ed25519 签名的客户端版本，可配置。

**B 的现状**

- 自有格式 `counterVbase64`，JSON 包装；另支持官方 `ts.ini`（这是 B 的优势）。
- `unique_id()` 固定 SHA-1（`crates/tslib-core/src/identity.rs:201-203`；`D/utils/tsproto-types/src/crypto.rs:246-253`）。
- 没有 capabilities/family/uid algorithm；`ServerInfo.uid` 从未赋值（`crates/tslib-core/src/state.rs:71-96`）。
- `improve` 没有 160 上限；`ClientConfig` 没有客户端版本字段。
- 导入裸 key 会自动升到 level 8（依赖层行为），与 A 的“保留原 offset”不同。

**建议**

1. `Identity::from_str` 同时接受 `offsetV...` 与 `base64:offset`；新增 `export_ts_string()`（A 兼容）与现有 `export_string()` 并存。
2. `Identity::uid(UidAlgorithm)`；`ServerCapabilities` 从 `initserver` 解析并暴露。
3. 安全等级上限 160 显式校验；升级过程支持取消（`tokio::select!`/`CancellationToken`）。
4. `ClientConfig.version: Option<ClientVersion>`，使用依赖层已有的版本枚举；默认值保持现状。
5. 如需 TS6 头像文件名，按 UID 长度选择 40/64 字母编码。

**验收**：与 A 的 identity 串双向导入；SHA-256 UID 测试向量；TS6 capabilities 解析测试。

---

### 1.11 可观测性：trace 阶段、脱敏、诊断事件

**A 的做法**

- `ClientTrace` 13 个 hook：`resolveStart/Done`、`connectStart/Done`、`handshakeDone`、`sessionStart`、`welcomeDone`、`attemptFailed`、`disconnectStart/Done`、`sessionEnd`、`reconnectScheduled/Stopped`（`A/src/client/trace.ts:216-247`）。
- `sessionStart` 会剔除密码、token、key offset 等敏感字段（`A/src/client/client.ts:270-276,1342-1356`）。
- `diagnostics_channel` 只在命令级发事件，不带参数内容。
- 每条尝试恰好一个 `sessionStart` 或 `attemptFailed`，每个 session 恰好一个 `sessionEnd`，这些不变式有测试。

**B 的现状**

- 只有 `tracing` 日志与 `get_network_stats()`；没有阶段耗时、没有 session 生命周期不变式、没有脱敏测试。
- `ClientConfig` 实现了 `Serialize`，`Identity` 的序列化/`Debug` 是否泄漏私钥需要单独确认。

**建议**

1. 用 `tracing` span 表达阶段：

```text
resolve → connect → handshake → session_start → welcome_done → disconnect → session_end
```

每个 span 带 `session_id`、`attempt`、`duration_ms`、`family`；失败带 `error_kind`。
2. 增加可选 `ClientTrace` trait 或回调，方便 JNI/Python 上报。
3. 所有包含密码/token/身份私钥的结构体实现自定义 `Debug`，统一打印 `[redacted]`；加单测。
4. 指标：RTT、丢包、收发字节、事件队列深度、丢弃数、重连次数、命令超时数。

**验收**：日志/诊断中不出现敏感字段；一次连接的生命周期事件成对出现。

---

### 1.12 测试与协议行为文档

**A 的做法**

- 大量 fuzz：地址、命令、通知、传输包、QuickLZ、接收处理。
- 集成测试在真实 TS3 3.13.8 / TS6 6.0.0-beta13.1 上跑，并把实测行为写进 `docs/voice-protocol-responses.md`、`docs/serverquery-responses.md`。
- 很多注释/文档直接写“实测于哪个版本、哪个命令返回什么”，例如 `ftstop` 在 TS6 会崩、`ftgetfilelist` 空目录返回 1281、`channelcreate` 可能只有自己的行才是应答。

**B 的现状**

- 单元测试覆盖配置、状态、BBCode、频道树、Opus 等；绑定层 0 测试；在线测试较浅（“Cannot send audio”被当通过）。
- 没有 fuzz；没有协议实测文档；README 的“35+ 事件/242 测试/TS3/6”与实际有出入。

**建议**

1. Fuzz 目标：命令构造/转义、QuickLZ 解压、UDP 包解析、identity 字符串、权限 hint 计算。
2. Golden tests：identity 两种格式、channel order、时间单位、权限 hints、错误码映射。
3. 集成矩阵：TS3 3.13.8、TS6 6.0.0-beta13.1；用 Docker/CI 起服务器；记录每个命令的实测响应。
4. 建立 `docs/protocol-behavior.md`，记录 B 依赖层与服务器的怪癖（1281、notification 配对、`ftstop` 崩溃窗口、`clientupdate` 不回显等）。
5. 修正 README 的过期数字，或改为“以代码为准”的表述。

**验收**：CI 有 fuzz smoke；协议文档与测试同源；README 不再夸大。

---

### 1.13 raw 命令逃生口

**A 的做法**

`client.exec(line)` 发原始命令行并返回行；`execNoWait` 不等应答。这让 SDK 没建模的命令也能用，同时仍走节流/超时/错误处理。

**B 的现状**

只有 `inner()`/`inner_mut()` 暴露 `tsclientlib::Connection`，调用方要自己构造 `OutCommand`、处理 `return_code`、读 `InMessage`，容易绕过状态机和错误处理。

**建议**

```rust
pub async fn exec_raw(&mut self, line: &str) -> Result<Vec<HashMap<String, String>>>;
pub async fn exec_raw_no_wait(&mut self, line: &str) -> Result<()>;
```

- 仍走令牌桶、超时、`ServerError` 映射。
- 文档标注“raw 命令不保证跨版本兼容”。
- 逐步用 typed 命令替代内部使用。

**验收**：能通过 `exec_raw("channelcreate ...")` 建频道并拿到 id。

---

## 2. 架构层面可借鉴

### 2.1 分层：协议核心 / 传输 / API / 绑定

A 的 `core` 被 `client`、`query`、`stream` 共享：命令构造、解析、ID、错误、事件名都在 core；client/query/stream 只做传输与语义差异。

B 目前把协议逻辑、状态、文件、权限、事件全部塞在 `tslib-core::Client`（1729 行），绑定层再各自拼装。建议拆成：

```text
tslib-protocol    // 命令构造/解析、常量表、时间、ID、错误
tslib-transport   // tsclientlib 适配、重连、限流、超时
tslib-client      // 高层 API、状态/view、事件
tslib-audio/chat/channel/bot
tslib-ffi/python/jni
```

短期不必新建 crate，可以先用模块边界约束依赖方向：`client` 不直接依赖 `tsclientlib` 类型，统一经过 `transport` 适配层。

### 2.2 用 trait 隔离可替换部分

```rust
#[async_trait]
pub trait CommandExecutor {
    async fn execute(&mut self, cmd: Command) -> Result<Vec<Row>>;
}

#[async_trait]
pub trait EventSource {
    async fn next_event(&mut self) -> Result<Option<Event>>;
}

pub trait TransportFactory {
    fn connect(&self, options: &ConnectOptions) -> Result<Box<dyn Transport>>;
}
```

这样未来可以：
- 用 mock transport 做离线测试；
- 加 ServerQuery/WebQuery（如果产品需要）；
- 让 Python/JNI 复用同一事件泵，而不是各自 `block_on`。

### 2.3 绑定层只做适配

当前 JNI/Python/FFI 各自实现事件转换、字段映射、默认值，容易出现 Java 缺 native、Python 事件负载被裁剪、FFI 无事件泵。建议：

- 在 `tslib-core` 定义“绑定友好的 DTO”（纯数据、无 tsclientlib 类型）。
- 绑定层只做 `DTO -> JNI/PyObject/Event` 的机械转换。
- 用一份 `bindings/api.json` 或 `cbindgen`/`pyo3`/`jni` 生成/校验声明，避免签名漂移。
- 每个绑定至少加一个端到端 smoke test（连接 mock server + 收发一条消息）。

---

## 3. 落地路线图

> 优先级按“数据正确性 > 协议对齐 > 新功能”。工作量 S/M/L 为粗略估计。
>
> **注意**：本章是“差距清单的分组”，不是最终执行顺序。第 8 节给出了经过评审的 Phase 0–4 路线、依赖关系与缺口补强；实际排期以第 8 节为准。

### P0：先修正确性（不修会交付错误结果）

| # | 任务 | 关键文件 | 工作量 | 验收 |
|---|---|---|---|---|
| 1 | 移除下载 10 MB 静默截断；改为可配置上限或流式；超限明确报错 | `crates/tslib-core/src/client.rs:565-606` | M | >10 MB 文件内容一致 |
| 2 | 文件传输超时改为 idle timeout，不再 5 s/10 s 总超时 | `client.rs:575,611` | S | 慢链路大文件可完成 |
| 3 | Bot 事件泵：独立任务调用 `wait_events`，喂 broadcast | `crates/tslib-bot/src/bot.rs:118-203` | S | 发消息能触发命令 |
| 4 | FFI 事件泵 + runtime；接线 callback | `crates/tslib-ffi/src/lib.rs:61-75,196-246` | M | C 示例能收事件 |
| 5 | 修 Java `moveToChannel` 签名；补 6 个 native 声明与 `EchoCanceller.java` | `java/.../Client.java:214,356`；`crates/tslib-jni/src/client.rs:587-602` | S | Java 调用不再 `UnsatisfiedLinkError` |
| 6 | 初始静音改为配置项（PTT 默认 vs 连接后解除） | `client.rs:438-441` | S | 两种策略可选 |
| 7 | `channel_order` 改 `u64`，频道树按前驱链排序 | `state.rs:112`；`client.rs:1635`；`tslib-channel/src/tree.rs:195-205` | M | 树顺序与官方客户端一致 |
| 8 | `is_away` 改用 `client_away` | `client.rs:1589` | S | away/留言组合正确 |
| 9 | 高频命令返回 `Result`（移动/消息/poke/文件操作） | `client.rs:1175-1451` | M | 失败可感知 |
| 10 | 修 `Connected` 发两次、`ConnectionStateChanged` 不发射 | `client.rs:186,426`；`connection.rs` | S | 事件序列正确 |

### P1：协议与 API 对齐

| # | 任务 | 关键文件 | 工作量 | 验收 |
|---|---|---|---|---|
| 11 | 协议常量表 + 权限 id 去硬编码 | 新增 `protocol/*`；`client.rs:1669-1681` | M | 无 magic number |
| 12 | 类型化 ID 与范围校验 | 新增 `types/id.rs`；公开 API 逐步替换 | M | 编译期防混用 |
| 13 | 缺失值三态 + `FieldUpdate` + `raw` 字段 | `state.rs:71-224` | M | 能区分缺失/false |
| 14 | 统一 `execute` + `ServerError` + `NoData` + 超时 | 新增 `command.rs`；`error.rs` | L | 命令失败有错误码 |
| 15 | 事件模型：`Notification` 兜底 + 每消费者缓冲 + 补齐 7 个事件 | `events.rs`；`client.rs:194-363` | L | 事件不丢/可观测 |
| 16 | 重连策略 + 令牌桶 + 524 重试 | `config.rs`；新增 `supervisor.rs`/`throttle.rs` | L | 故障注入通过 |
| 17 | 文件续传/密码/`ftstop`/`ftgetfileinfo`/`ftlist`/`proto` | `client.rs:1356-1451` | L | 续传与加密频道可用 |
| 18 | Identity 双格式 + SHA-256 UID + 160 上限 | `identity.rs:179-260` | M | 跨 SDK 互导 |
| 19 | `ServerCapabilities` + `ClientConfig.version` | `state.rs`；`config.rs` | M | TS6 能力可读 |
| 20 | 时间解析统一（秒/ms/us/ns） | 新增 `protocol/time.rs`；`client.rs:913` | S | TS6 时间正确 |
| 21 | 订阅策略可配置 + 重连后恢复 | `client.rs:412-467,551-557` | S | 重连后订阅仍在 |
| 22 | `exec_raw` 安全逃生口 | `client.rs` | S | 未建模命令可用 |

### P2：新能力（按产品边界决定）

| # | 任务 | 说明 | 工作量 |
|---|---|---|---|
| 23 | 完整 server view + revision + ready/changed | 替代部分 `ServerState` 镜像 | L |
| 24 | 频道/客户端/群组/权限/封禁命令层 | 参考 A 的 9 个模块，按实际需求裁剪 | XL |
| 25 | 头像/图标全生命周期 | 上传/下载/删除/文件名推导 | M |
| 26 | ServerQuery crate | TCP/SSH/WebQuery；若只做语音客户端可不做 | XL |
| 27 | TS6 Streams crate | 屏幕共享信令；若做 TS6 客户端则是硬缺口 | XL |
| 28 | 版本表/选择器 | 依赖层已有枚举，先暴露配置即可 | S |
| 29 | trace/diagnostics | 阶段 span + 脱敏 + 指标 | M |
| 30 | fuzz + 协议实测文档 | 长期质量基础设施 | M |

---

## 4. 不建议照搬的部分

| A 的做法 | 为什么不建议照搬 | B 应采取的做法 |
|---|---|---|
| 只做协议帧，不含 Opus/音频处理 | B 的音频链是差异化优势 | 保留并补齐 jitter/重采样/AEC 集成 |
| 语音/whisper 恒明文 | `codec_encryption_mode=ForcedOn` 下可能被拒；也不符合“按服务器设置” | **不改 D 只能固定加密**；记录为限制。若要跟随服务器，需自研发包路径（暂不做） |
| License 只校验 version 1、不校验时间窗 | 会接受过期/越界 license | 保留依赖层的严格校验（version 0/1 + 时间窗 + bounds） |
| 握手/连接无总超时，只靠 60 s idle | 卡死时体验差 | 保留可配置的连接/握手超时 |
| 命令窗口 1024、语音窗口 32768、1 MB 队列 | 参数针对 Node 场景，B 的依赖层已有拥塞控制 | 不照抄数字；B 加 watchdog 缓解空洞卡死（不改 D） |
| `ftstop` 只在 5 s 窗口内发 | 是 TS6 崩溃的规避，不是通用规则 | 记录为 TS6 quirk，按服务器版本分支 |
| ServerQuery 整体 `@experimental` | API 可能小版本变化 | 若做，先冻结内部接口，不要直接暴露给绑定层 |
| TS6 Stream 协议未文档化 | 服务器升级可能失效 | 若做，加能力门禁 + 版本探测 + 失败降级 |
| Node 22/ESM/AbortSignal 形态 | 运行时不同 | Rust 用 `CancellationToken`/`tokio::time::timeout`，绑定层映射为各自的取消机制 |
| 单包 500 B、语音不分片 | 协议约束，不是设计优点 | 保持依赖层现有分片逻辑，不要绕过 |

---

## 5. 需要拍板的决策

以下不是技术细节，而是产品/兼容性取舍，建议逐项确认后再动手：

### 5.1 产品边界

- **只做 TS3/TS6 语音客户端**：P2 的 ServerQuery/Streams 可不做，把精力放在 P0/P1。
- **加管理机器人**：需要 ServerQuery（TCP/SSH/WebQuery），工作量 XL。
- **加 TS6 屏幕共享**：需要 Streams 信令，工作量 XL，且协议未文档化。
- **做通用 SDK 对标 A**：需要 P1 全部 + P2 的命令层/view/版本表。

### 5.2 Identity 兼容

- 选项 A：只保留 B 现有格式 + `ts.ini`，不兼容 A。
- 选项 B：`from_str` 同时接受 `base64:offset`，`export` 增加 A 兼容格式（推荐，成本 S）。
- 选项 C：统一改成 A 格式，破坏现有用户（不推荐）。

### 5.3 初始静音

- 选项 A：保持“连接后默认 input muted”（适合 PTT/Android 客户端）。
- 选项 B：连接后显式解除（对齐 A，适合语音常开）。
- 选项 C：配置项，默认保持现状（推荐）。

### 5.4 语音加密

- 选项 A：固定加密（现状；**不改 D 的唯一可行选项**），文档记录限制。
- 选项 B：自研发包路径以控制加密开关（成本高，暂不做）。
- 选项 C：按 `codec_encryption_mode` + 频道标志决策（需要改 D，按当前约束不考虑）。

### 5.5 文件传输形态

- 选项 A：保留内存 API，去掉 10 MB 截断，加上限报错（最小改动）。
- 选项 B：新增流式 API，内存 API 作为便捷封装（推荐，兼容旧调用方）。
- 选项 C：只做流式，破坏现有 API。

### 5.6 事件 API

- 选项 A：保留 `broadcast`，只补 `Notification` 与缺失事件（兼容）。
- 选项 B：新增每消费者 `EventStream`，`broadcast` 标记 deprecated（推荐，渐进迁移）。
- 选项 C：直接替换为 mpsc，破坏现有绑定（不推荐）。

### 5.7 频道顺序

- 选项 A：只把 `order` 改 `u64`，树仍按数值排（错误依旧）。
- 选项 B：按前驱链重建顺序，Java 侧已有的正确实现可移植到 Rust（推荐）。

### 5.8 TS6 目标

- 选项 A：只保证“能连兼容 TS3 协议的 TS6 服务器”。
- 选项 B：补 capabilities/UID SHA-256/TS6 命令。
- 选项 C：完整 TS6（含 Streams）。

---

## 6. 验收与测试清单

### 6.1 单元/黄金测试

- [ ] Identity：`offsetVbase64`、`base64:offset`、`ts.ini` 三种格式双向；私钥不出现在 `Debug`/`Serialize`。
- [ ] UID：SHA-1/SHA-256 已知向量。
- [ ] 时间：秒/毫秒/微秒/纳秒/0/负数。
- [ ] 频道树：前驱链顺序、根/子、删除父节点、环检测。
- [ ] 权限 hints：12 个权限 id 的边界组合。
- [ ] 命令序列化：转义、密码字段、`FieldUpdate` 只发已设置字段。
- [ ] 错误映射：`ServerError` 的 code/extra/failed_permid、1281、512/768。

### 6.2 集成/故障注入

- [ ] TS3 3.13.8 与 TS6 6.0.0-beta13.1 连接/文本/语音/文件。
- [ ] 文件 >10 MB 往返、断点续传、空闲超时、加密频道。
- [ ] 断网/超时/524：重连退避、限流、`will_reconnect`。
- [ ] 事件顺序：state 更新先于事件；typed 先于 notification。
- [ ] 慢消费者：不阻塞 pump，`dropped` 递增。
- [ ] 重连后：订阅恢复、静音状态一致、server variables 刷新。

### 6.3 绑定 smoke

- [ ] JNI：连接 mock/真实服务器、收文本、收语音、文件下载、`moveToChannel`。
- [ ] Python：连接、收发消息、事件字典字段完整。
- [ ] FFI：C 示例连接、收事件、断开。

### 6.4 质量门禁

- [ ] `cargo clippy --workspace -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `cargo build --workspace --release`
- [ ] Android `cargo ndk` 构建
- [ ] 协议常量重新生成无 diff
- [ ] 无新增 `unwrap()`/裸 `u16` client id/硬编码权限 id

---

## 7. 参考索引

### A（teamspeak-js）

| 主题 | 文件 |
|---|---|
| 客户端生命周期/命令/事件 | `A/src/client/client.ts` |
| 事件类型 | `A/src/client/types.ts` |
| 传输/重传/语音包 | `A/src/client/transport/handler.ts` |
| 加密/EAX/握手 | `A/src/client/crypto/*`、`A/src/client/handshake.ts` |
| 身份 | `A/src/client/crypto/identity.ts` |
| 文件传输 | `A/src/client/transfer.ts`、`A/src/client/api/files.ts`、`upload-checks.ts` |
| 语音/订阅/静音 | `A/src/client/api/voice.ts` |
| 服务器视图 | `A/src/client/view.ts`、`A/src/core/channel-tree.ts` |
| 命令 API | `A/src/core/api/*` |
| 重连/限流/错误 | `A/src/core/reconnect.ts`、`A/src/client/throttle.ts`、`A/src/core/errors.ts` |
| 能力/版本 | `A/src/client/capabilities.ts`、`A/src/versions/index.ts` |
| ServerQuery | `A/src/query/*` |
| TS6 Streams | `A/src/stream/*` |
| 文档 | `A/docs/guide.md`、`configuration.md`、`design.md`、`voice-protocol-responses.md` |

### B（当前项目）

| 主题 | 文件 |
|---|---|
| Client 主实现 | `crates/tslib-core/src/client.rs` |
| 配置 | `crates/tslib-core/src/config.rs` |
| 事件 | `crates/tslib-core/src/events.rs` |
| 状态类型 | `crates/tslib-core/src/state.rs` |
| 身份 | `crates/tslib-core/src/identity.rs` |
| 错误 | `crates/tslib-core/src/error.rs` |
| 频道树 | `crates/tslib-channel/src/tree.rs`、`manager.rs` |
| 音频 | `crates/tslib-audio/*`、`crates/tslib-audio-aec3/*` |
| 聊天/BBCode | `crates/tslib-chat/*` |
| Bot | `crates/tslib-bot/*` |
| JNI/Java | `crates/tslib-jni/*`、`java/src/main/java/dev/tslib/*` |
| FFI/Python | `crates/tslib-ffi/*`、`crates/tslib-python/*` |

### D（依赖层，rev `3fbfa26`）

| 主题 | 文件 |
|---|---|
| 连接/事件流/重连 | `D/tsclientlib/src/lib.rs` |
| 握手/puzzle | `D/tsproto/src/client.rs` |
| 加密/加密策略 | `D/tsproto/src/algorithms.rs`、`connection.rs`、`packet_codec.rs` |
| 接收窗口 | `D/tsproto/src/connection.rs` |
| 重传/超时 | `D/tsproto/src/resend.rs` |
| License | `D/tsproto/src/license.rs` |
| UID | `D/utils/tsproto-types/src/crypto.rs` |
| 文件传输命令 | `D/tsclientlib/src/lib.rs` |
| 解析器 | `D/tsclientlib/src/resolver.rs` |

---

## 8. 计划评审：它是最佳实践吗？

### 8.1 评审结论

| 维度 | 评分（10 分制） | 说明 |
|---|---:|---|
| 差距识别 | 8 | 主要功能面基本覆盖，证据充分 |
| 证据质量 | 8 | 双侧源码 + 依赖层核对，少量线上行为待实测 |
| 优先级 | 5 | 按“正确性 > 协议 > 新功能”分组合理，但没有产品价值/成本排序 |
| 依赖与排序 | 3 | P0/P1/P2 不是依赖图；多项任务互为前置 |
| 架构设计 | 4 | 没有解决 `Client` 非 Send、actor、绑定线程模型 |
| 兼容策略 | 3 | 破坏性 API 改动没有 semver/迁移/ABI 方案 |
| 非功能要求 | 2 | 安全、性能、功耗、内存、可靠性没有预算 |
| 交付与测试 | 4 | 有验收点，但缺 CI、真实服务器、fuzz、打包 |
| 风险与回滚 | 2 | 没有风险登记、回滚、feature flag |
| **综合** | **约 4.5/10** | **合格的差距分析，不是可执行的最佳实践计划** |

**八个结构性问题**

1. **基准错位**：以“A 有什么”为驱动，而不是以“B 的产品目标/用户价值”为驱动。A 的 ServerQuery、Streams 是 A 的产品选择，不一定是 B 的。
2. **依赖层边界未分析**：没有说明在不改 `tsclientlib/tsproto` 的前提下，哪些能修、哪些只能绕行、哪些只能接受。**当前决定：不 fork、不打补丁**，因此必须把受限项写清楚。
3. **并发模型缺失**：`Client` 是 `!Send/!Sync`，FFI/Python/JNI 事件泵与后台重连需要明确线程归属。P0 可先用“同线程 `wait_events`/专用线程持有 Client”解决；完整 actor 可在性能阶段再做。
4. **兼容策略缺失**：类型化 ID、返回 `Result`、事件模型、identity 格式都是破坏性变更；没有 semver、deprecation、feature flag、迁移、ABI 版本。
5. **非功能要求缺失**：安全、性能、功耗、内存、可靠性、隐私、可观测性没有预算和验收。
6. **交付基础设施缺失**：真实 TS3/TS6 CI、录制回放、fuzz、sanitizer、cargo-deny、SBOM、AAR/wheel/header 打包、发布流程。
7. **排序不是执行顺序**：actor → 事件流；command executor → typed commands；file handle → streaming；event model → view。原计划没有画出这些前置关系。
8. **没有“不做什么”**：P2 的 XL 项没有产品门槛，容易范围蔓延。

**结论**：不要直接按第 3 节的 P0/P1/P2 开工。当前按第 9 节的批次执行（错误修复 → 性能优化）；第 8 节保留为评审背景，涉及改依赖层的建议已按“不 fork”约束改写。

---

### 8.2 六个结构性缺口与补强

| # | 缺口 | 当前计划的状态 | 后果 | 补强动作 |
|---|---|---|---|---|
| 1 | 产品目标与范围 | 只按 A 的功能面分组 | 做了不需要的功能，漏了真正的用户价值 | 写一页产品目标 + MoSCoW；明确“只做语音客户端 / 加管理 / 加 TS6” |
| 2 | 依赖层策略 | 未区分“能修/只能绕行/只能接受” | 误以为所有问题都能修 | **已决定不 fork、不改 D**；只使用公开 API；受限项见 8.5/9.3 |
| 3 | 并发模型 | 未提及 | `!Send` 与绑定/重连冲突 | P0：同线程 `wait_events` 或专用线程持有 `Client`；完整 actor 作为性能阶段可选项 |
| 4 | 兼容策略 | 未提及 | 升级即破坏 Android/Python/C 调用方 | semver、deprecation、feature flag、双 API、ABI 版本、迁移测试 |
| 5 | 非功能要求 | 只有零散验收点 | 上线后音频卡顿/耗电/崩溃/泄漏 | 安全、性能、功耗、内存、可靠性预算与基准 |
| 6 | 交付基础设施 | 只有“加测试” | 无法持续验证、无法回归、无法发布 | CI 矩阵、真实服务器、录制回放、fuzz、sanitizer、打包与发布 |

---

### 8.3 遗漏的功能清单（回答“是否包含所有需要新增/修改”）

**覆盖度自查**

| 类别 | 原计划覆盖 | 主要缺口 |
|---|---|---|
| 协议/连接 | 部分 | 组 whisper、语音 mute 列表、单频道订阅/退订、密码校验、插件命令、输入状态、server variables、raw exec、版本选择、capabilities |
| 事件/状态 | 部分 | `notification` 兜底、`viewReady/viewChanged`、事件顺序契约、慢消费者策略、7 个未发射事件 |
| 命令 | 部分 | 频道 CRUD、踢/封禁、群组、权限写、临时密码、privilege key、日志/投诉/离线消息、分页/批量 |
| 文件 | 部分 | 流式、续传、进度、取消、`ftstop`、`ftgetfileinfo`、`ftlist`、递归列表、上传校验、频道密码、`proto` |
| 头像/图标 | 部分 | 上传/下载/删除、文件名推导、大小权限 |
| **音频** | **几乎未覆盖** | **AEC 集成、重采样、jitter buffer、PLC 接线、codec 协商、AGC/NS、设备格式、时钟漂移、解码器清理、实时分配、audio focus、蓝牙、网络切换** |
| 绑定 | 部分 | actor handle、FFI 事件泵、Java 缺失 native/EchoCanceller、Python GIL/block_on、ABI 版本、AAR/wheel/header |
| 安全/隐私 | 未覆盖 | 私钥存储、zeroize、日志脱敏、cargo-deny、SBOM、供应链 |
| 可观测性 | 部分 | 指标、阶段 trace、队列深度、丢弃计数、重连原因 |
| 发布/文档 | 未覆盖 | 迁移指南、每绑定示例、AAR/wheel/header、changelog、版本策略 |

**8.3.1 协议/连接补充**

- 组 whisper（server group / channel group / commander / all clients）与目标校验。
- `clientmute`/`clientunmute`（监听者侧语音屏蔽）。
- 单频道 `channelsubscribe`/`channelunsubscribe`、`verifyserverpassword`/`verifychannelpassword`。
- `sendPluginCommand`、`clientChatComposing`、`clientChatClosed`。
- `getServerVariables` 的 typed 返回，而不是只依赖 `notifyserverupdated`。
- `exec_raw`/`exec_raw_no_wait`（已列 P1，但需明确错误/节流语义）。
- `ClientConfig.version` 与服务器最低版本处理。
- `ServerCapabilities`（family、uid algorithm、extensions、server UID）。
- `Event::Notification` 兜底与原始字段。
- 每命令超时/取消；524 等待重试；令牌桶。
- 重连后的会话恢复：订阅、静音、server variables、voice seq、client id 变化。
- 空结果集语义（1281、512、768）。
- 时间单位统一（秒/ms/us/ns）。
- `maxclients` 的 unlimited/inherited 区分。
- `channel_order` 前驱链语义。
- `client_away` 判定与 `client_away_message` 分离。

**8.3.2 管理命令补充**

- 频道：create/edit/delete/move、频道权限、频道内客户端权限。
- 客户端：kick、ban、move、edit、数据库分页、UID/DBID 查询、自定义属性。
- 群组：服务器组/频道组 CRUD、成员、权限、复制、排序。
- 权限：`permget`、`permfind`、权限目录、own permissions、权限重置。
- 管理：封禁列表/`banfind`、投诉、离线消息、日志、临时密码、privilege key、API key、query login。
- 批量：多行 `clientmove`/`clientkick`/权限增删 + `-continueonerror`。
- 分页/迭代：`pages()`、`iterateClientDatabase()`、`listBans(start/limit)`。
- `channelcreate` 只取自己回显行的语义（避免拿到别人的广播）。

**8.3.3 文件/头像/图标补充**

- 流式上传/下载（`Readable`/`Writable` 对应 Rust 的 `AsyncRead`/`AsyncWrite`）。
- 续传：上传 `resume`、下载 `seek_position`。
- 进度回调、取消、`ftstop`、`ftgetfileinfo`、`ftlist`、递归列表。
- 上传后大小校验；失败清理的 5 秒窗口。
- 频道密码透传；`proto` 可配置。
- 头像上传/下载/删除、`avatar_<uid nibble>` 文件名、TS3 40/TS6 64 字母。
- 图标 CRC32 id、`/icon_<id>`、按权限设置各类图标。
- 文件权限 hints 与服务器实际权限的一致性。

**8.3.4 音频（原计划最大的功能遗漏）**

| 缺口 | 现状 | 影响 | 建议优先级 |
|---|---|---|---|
| AEC 未接入 AudioManager | `echo_cancellation` 配置从未读取；AEC3 只在 JNI 单独使用 | Android 外放回声 | P0/P1 |
| 无重采样 | `rubato` 声明未用；设备非 48 kHz 直接失败 | 大量设备无法采集/播放 | P1 |
| 无 jitter buffer | 固定环形缓冲，写入覆盖未读 | 网络抖动直接爆音/丢字 | P1 |
| PLC 未接线 | `decode_plc` 只在 JNI 暴露，`handle_packet_loss` 无调用方 | 丢包时静音/杂音 | P1 |
| codec 被忽略 | `process_incoming` 一律 Opus 解码 | 非 Opus 频道音频错误 | P0/P1 |
| AGC/NS 未接线 | 配置存在但 0 读取 | 音量/噪声体验差 | P2 |
| 设备格式未协商 | 强制 48 kHz/单声道 | 设备打开失败 | P1 |
| 时钟漂移 | 采集/播放独立时钟，无补偿 | 长时间通话漂移/欠载 | P1 |
| 解码器不清理 | `decoders` 只增不减 | 长会话内存增长 | P1 |
| 实时回调分配 | playback/mixer 每次回调 `Vec` 分配 | 爆音/抖动 | P1 |
| audio focus/蓝牙/网络切换 | 无 | Android 体验/断线 | P1/P2 |
| 音频指标 | 无 | 无法定位质量问题 | P1 |

**8.3.5 绑定补充**

- actor/handle：绑定层不再直接持有 `!Send` 的 `Client`。
- FFI：runtime、事件泵、回调注册/注销、线程安全声明、ABI 版本。
- Java：补齐 6 个 native、`EchoCanceller.java`、`waitEvents`、`setAudioSink`、`sendPoke`、`getNetworkStats`、`setWhisperTargets`。
- Python：`block_on` 与 GIL 的交互（`allow_threads`/专用线程）、事件字典字段完整、`ChatHistory.add`。
- 打包：Android AAR + 16 KB page size、Python wheel、C header + `tslib_abi_version`。
- 绑定 smoke/E2E 测试。

**8.3.6 安全/隐私/运维补充**

- 私钥落盘：Android Keystore/文件权限；`zeroize` on drop；不进入日志/崩溃报告。
- 密码/token/`client_key_offset` 脱敏；`Debug`/`Serialize` 测试。
- `cargo-deny`/`cargo-audit`、SBOM、依赖锁定与升级策略。
- `unsafe` 策略：安全 crate `deny(unsafe_code)`，FFI/AEC 局部 allow。
- 指标：RTT、丢包、队列深度、丢弃事件、重连次数、命令超时。
- 日志级别与隐私：生产默认 info，不记录消息正文。
- 供应链：vendored AEC3/speex/Opus 的许可证与更新。

**8.3.7 发布/文档补充**

- 迁移指南：identity 格式、`Result` 返回、事件 API、类型化 ID。
- 每个绑定一个可运行示例。
- `docs/protocol-behavior.md`：服务器/版本相关的实测行为。
- CHANGELOG、semver、发布流程、制品校验。
- README 修正过期数字与能力宣称。

---

### 8.4 遗漏的工程实践

| 实践 | 为什么需要 | 产出 |
|---|---|---|
| MoSCoW | 防止 P2 的范围蔓延 | `docs/scope.md`：Must/Should/Could/Won't |
| ADR | 记录关键架构选择与理由 | `docs/adr/0001-actor.md`、`0002-dependency.md`、`0003-compat.md`、`0004-audio.md` |
| RFC（破坏性变更） | Android/Python/C 调用方需要评审 | 每个 breaking change 一份短 RFC |
| Spike | 协议行为未实测，先验证再排期 | 时间盒 1–3 天的实验分支/记录 |
| Feature flag | 默认静音、语音加密、identity 格式需要灰度/回滚 | `ClientConfig` 开关 + 运行时配置 |
| 依赖图 | P0/P1 不是执行顺序 | 任务前置关系图（见 8.9） |
| 风险登记 | 依赖 pin、TS6、音频设备、API 破坏 | 风险表（见 8.10） |
| DoD | “完成”不能只有代码 | 代码 + 测试 + 文档 + changelog + flag + 指标 |
| 性能预算 | 音频/移动端对性能敏感 | 基准与门槛（见 8.8） |
| 安全评审 | 私钥、密码、供应链 | threat model + checklist |
| CI 矩阵 | 多平台/多绑定 | Windows/Linux/macOS + Android arm64/x86_64 + Python/Java |
| 真实服务器夹具 | mock 不能覆盖协议怪癖 | Docker TS3/TS6 + 录制回放 |
| Fuzz/sanitizer | 解析器/压缩/包处理是攻击面 | cargo-fuzz + ASan/UBSan |
| `cargo-semver-checks` | 防止无意破坏 API | CI 任务 |
| 发布工程 | AAR/wheel/header 是产品的一部分 | 发布脚本 + 制品校验 |

---

### 8.5 依赖层边界（已决定：不改 tsclientlib）

**约束**

- 不 fork、不打 patch、不 vendored 修改 `tsclientlib/tsproto`。
- 只使用依赖层公开 API；可以启用已有 feature（如 `unstable`），但不改源码。
- 需要改 D 才能修的问题，只能：B 层绕行、接受现状、或自研替代模块（成本另算）。

**可用的公开 API（决定了很多事不用改 D）**

| 能力 | 公开入口 | 能解决什么 |
|---|---|---|
| 命令结果关联 | `OutCommandExt::send_with_result(con)` + `StreamItem::MessageResult(MessageHandle, Result)`（`D/tsclientlib/src/lib.rs:70-71,230,1621-1626`） | typed command、错误反馈、超时 |
| 连接选项 | `ConnectOptions::version/hardware_id/channel_password/input_muted/output_muted/away`（`D/tsclientlib/src/lib.rs:1830-1967`） | 修 `ClientConfig` 死字段、频道密码、版本选择 |
| 服务器公钥 | `Connection::get_server_key()`（`D/tsclientlib/src/lib.rs:839`） | 计算 server UID / SHA-256 |
| 组 whisper 包 | `AudioData::C2SWhisperNew`（`D/utils/tsproto-packets/src/packets.rs:220-227`） | 组 whisper |
| 服务器加密模式 | `data::Server.codec_encryption_mode`（生成 `structs.rs`） | 至少能“知道”，但不能改发包策略 |
| InitServer 扩展字段 | 生成的 `InInitServer.capability_extensions` / `virtualserver_unique_identifier` | 若 B 能拿到该消息，可解析 TS6 能力 |
| 公钥编码 | `EccKeyPubP256::to_ts()`（`D/utils/tsproto-types/src/crypto.rs:218`） | B 自己算 SHA-256 UID |

**D 层限制（不改则无法修复，只能绕行/接受）**

| # | 问题 | D 的行为 | B 层可行方案 | 结论 |
|---|---|---|---|---|
| 1 | 语音加密模式 | `voice_encryption` 固定 `true`（`D/tsproto/src/connection.rs:142`） | 无 | **接受“始终加密”**；文档记录；若服务器强制明文则无法支持 |
| 2 | 乱序包提前 ACK / 无空洞超时 | 收到即 ACK；无 hole timeout（`D/tsproto/src/packet_codec.rs`） | B 加 watchdog：长时间无事件/无 pong 主动重连 | **缓解**，不能根治 |
| 3 | TS6 文件时间 | 按秒解析且不保留原始值 | 自研文件列表命令（成本高） | **接受/记录**；TS6 文件时间不可用 |
| 4 | Init0 version / 握手 step 0/2 / puzzle 上限 | D 固定实现 | 无 | **接受**（实测服务器一般不拒绝） |
| 5 | 文件 `proto=1` | 硬编码 1（`D/tsclientlib/src/lib.rs:1645,1669`） | 自研文件传输（命令 + TCP） | **暂接受**；若实测 IPv6 有问题再评估自研 |
| 6 | `capability_extensions` 未进 book | `Server` 结构无该字段 | `serverinfo` 的 `InServerInfo` 未声明属性，可能拿不到；`InitServer` 又被 D 内部消费 | **大概率受限**；尝试后若失败则记录，不做承诺 |
| 7 | `clientek` 假加密判定 | 依赖首包 id/前缀 | 无 | **接受** |
| 8 | identity 裸 key 自动升级 level 8 | `new_from_str` 行为（`D/tsproto/src/lib.rs:177-187`） | B 自己解析裸 key 后 `Identity::new(key, offset)` | **可解** |
| 9 | `away` 独立标志 | book 只有 `away_message` | 无；只能启发式 | **接受**（现逻辑基本可用） |
| 10 | `channel_order`/`maxclients`/`User` 可选字段 | 结构里都有 | B 映射即可 | **可解** |

**结论**：错误修复与性能优化中，绝大多数不需要改 D；真正受限的只有语音加密、TS6 文件时间、接收窗口缺陷、文件 `proto`、`clientek` 判定。计划不再包含 fork/upstream/patch。

---

### 8.6 并发模型：actor + handle（性能阶段可选项，不是 P0 前置）

> 当前“错误修复 + 性能优化”不要求先做完整 actor。P0 可用“同线程 `wait_events`”或“专用线程持有 `Client` + C 侧轮询”解决；完整 actor 主要服务于多消费者事件流、FFI/Python 长期运行与性能隔离，放在 Batch 4 之后评估。

**现状问题**

- `Client` 注释明确“not Send/Sync due to tsclientlib limitations”（`crates/tslib-core/src/client.rs:3-4`）。
- FFI/Python/JNI 各自 `block_on`，无法与后台事件泵、重连、每消费者事件流共存。
- `audio_sink` 在事件泵线程同步调用用户回调，慢回调会阻塞整个连接。
- `broadcast` 容量 256，控制事件也可能被丢弃，且没有 lag 处理。

**建议架构**

```text
┌──────────────┐   mpsc<Command>    ┌──────────────────────────┐
│ ClientHandle │ ─────────────────▶ │ ConnectionActor          │
│ Clone+Send   │ ◀───────────────── │  owns TsConnection       │
│ +Sync        │   oneshot<Result>  │  runs on dedicated thread│
└──────┬───────┘                    │  LocalSet / block_on     │
       │                            └──────────┬───────────────┘
       │ subscribe events                      │ publish
       ▼                                       ▼
┌──────────────┐                    ┌──────────────────────────┐
│ EventStream  │ ◀───────────────── │ EventBus (per-sub mpsc)  │
│ bounded mpsc │                    │ control: no loss         │
│ drop counter │                    │ voice: drop-oldest       │
└──────────────┘                    └──────────────────────────┘
```

- `TsConnection` 只在 actor 线程创建/使用；actor 线程用 `current_thread` runtime + `LocalSet`（或 `block_on`），保证 `!Send` 类型不跨线程。
- `ClientHandle` 是 `Clone + Send + Sync`，公开 API 全部走它；命令带 `oneshot` 返回 `Result`。
- 事件总线为每个订阅者维护独立有界队列；控制事件不丢（满时反压 actor），语音事件 drop-oldest 并计数。
- `audio_sink` 改为“有界队列 + 专用音频线程”，不阻塞 actor。
- 绑定层只持有 `ClientHandle`；JNI/Python/FFI 不再直接 `block_on` 连接对象。

**ADR 需要回答**：actor 线程数（每客户端一个 vs 全局一个）、关闭顺序、panic 隔离、命令取消、事件反压、重连时 handle 是否保持稳定、多客户端资源上限。

---

### 8.7 兼容策略

| 变更 | 破坏性 | 策略 |
|---|---|---|
| 类型化 ID | 编译期破坏 | 0.x 可接受，但提供 `From<u64>`/`Into<u64>` 过渡；绑定层用原始类型 |
| 命令返回 `Result` | 签名破坏 | 新增 `try_*` 或 feature flag；旧方法标记 `#[deprecated]` |
| 事件模型 | 行为/API 破坏 | 保留 `broadcast`，新增 `EventStream`；双轨一个 minor 周期 |
| Identity 格式 | 数据兼容 | 只增加解析能力，不改变默认导出；迁移测试 |
| 默认静音/语音加密 | 行为破坏 | 配置开关，默认保持现状；发布说明 |
| JNI/FFI ABI | 二进制破坏 | `tslib_abi_version`；结构体 `#[repr(C)]` + size/version 字段；Java 类同步 |
| Python API | 运行时破坏 | 版本号 + 弃用周期；保留旧方法 |

- 明确 semver 政策：0.x 允许 breaking，但需 CHANGELOG + 迁移说明；1.0 后严格。
- 每个 breaking change 一个 RFC，列出调用方影响与迁移代码。
- CI 跑 `cargo-semver-checks` 与 `cargo public-api` diff。

---

### 8.8 非功能要求与预算（初始值，需实测校准）

| 领域 | 指标 | 初始目标 | 测量方式 |
|---|---|---|---|
| 连接 | 局域网加入时间 | < 3 s p95 | trace span |
| 重连 | 断线恢复成功率 | > 95%（非封禁场景） | 故障注入 |
| 音频 | 端到端延迟（含固定 jitter 60 ms） | p95 < 120 ms | 回环测试 |
| 音频 | AEC ERLE | ≥ 15 dB 静态 / ≥ 12 dB 漂移 | 现有 echo_spike |
| 音频 | 丢包隐藏 | 5% 丢包无明显断续 | 网络损伤测试 |
| 性能 | 中端 Android CPU | 通话 < 5% | perfetto |
| 内存 | 单客户端常驻 | < 100 MB | Android profiler |
| 可靠性 | crash-free sessions | > 99.5% | 线上/测试 |
| 事件 | 控制事件丢失 | 0 | 单元/集成 |
| 事件 | 语音丢弃率 | < 1% | dropped 计数 |
| 文件 | 吞吐 | > 10 MB/s（LAN） | 基准 |
| 构建 | 矩阵全绿 | 100% | CI |

---

### 8.9 修订路线（Phase 0–4）

> 原 P0/P1/P2 是主题分组；下面是按依赖关系重排的执行顺序。工期为粗略估计，需按人数校准。

#### Phase 0：决策与验证（1–2 周）

**目标**：消除“未知”，锁定架构与范围，之后不再边做边改方向。

- [ ] 产品目标与 MoSCoW（语音客户端 / 管理 / TS6）。
- [ ] ADR-001 actor + handle。
- [ ] ADR-002 依赖层边界与绕行（**已决定：不 fork、不改 D**）。
- [ ] ADR-003 兼容策略（semver/flag/ABI）。
- [ ] ADR-004 音频链（AEC/jitter/resampler/PLC 的集成方式）。
- [ ] Spike：确认语音加密现状与服务器兼容性（只记录，不修改 D）。
- [ ] Spike：确认文件 `proto=1` 在 IPv4-only 环境的影响（若严重再评估自研传输）。
- [ ] Spike：确认 TS6 文件时间/`ftstop` 崩溃窗口的实际影响（接受或绕行）。
- [ ] Spike：只评估官方上游 `tsclientlib` 新版本可用性（不改代码）。
- [ ] 性能基线：event loop、状态同步、mixer、采集/播放、解码的 benchmark。
- [ ] 测试夹具：mock server + 录制回放 + Docker TS3/TS6 + CI 骨架。

**退出标准**：四个 ADR 通过；每个 spike 有结论；范围文档签字。

#### Phase 1：正确性 + 架构骨架（2–4 周）

- [ ] actor + `ClientHandle` + 命令/事件通道。
- [ ] 统一 `execute` + `ServerError`/`NoData`/`Timeout`（用公开的 `send_with_result`，不改 D）。
- [ ] 移除文件 10 MB 截断；改 idle timeout；流式/续传。
- [ ] Bot 事件泵；FFI 事件泵；Java `moveToChannel` 签名。
- [ ] `channel_order` + 树排序；默认静音配置；验证 `away` 启发式。
- [ ] `Connected` 去重；补齐最容易的 kick/ban 事件。
- [ ] 音频 P0：codec 协商、AEC 接入 AudioManager、PLC 接线、jitter buffer。
- [ ] 删除死代码/死配置；README 修正。

**退出标准**：P0 验收全绿；桌面 + Android 构建矩阵绿；bot 能真实触发命令。

#### Phase 2：协议/事件对齐（4–8 周）

- [ ] 协议常量表 + 权限 id 去硬编码。
- [ ] 类型化 ID + `FieldUpdate` + `raw` 字段。
- [ ] `Event::Notification` + 每消费者 `EventStream` + 慢消费者策略。
- [ ] `view` + revision + `view_ready`/`view_changed`。
- [ ] 重连策略 + 令牌桶 + 524 重试 + per-command timeout。
- [ ] 文件续传/密码/`ftstop`/`ftgetfileinfo`/`ftlist`；`proto` 记录为 D 层限制。
- [ ] Identity 双格式 + SHA-256 UID + 160 上限；capabilities + version。
- [ ] 时间解析统一；空结果集；订阅恢复；`exec_raw`。

**退出标准**：与 A 的共有功能矩阵达到“语义一致或有明确差异记录”；集成测试覆盖 >80% 的 P1 项。

#### Phase 3：命令面 + 音频硬化 + 绑定（6–12 周）

- [ ] 按 MoSCoW 实现命令层（频道/客户端/群组/权限/管理）。
- [ ] 头像/图标全生命周期。
- [ ] 音频：重采样、jitter buffer、AGC/NS、设备格式、时钟漂移、解码器清理、实时分配、audio focus/蓝牙/网络切换。
- [ ] 绑定：Java 缺失 native/EchoCanceller、Python GIL、FFI 回调/ABI。
- [ ] 打包：AAR、wheel、header、版本号、changelog。
- [ ] 性能/功耗/内存基准；Android 真机回归。

**退出标准**：端到端在 Android 真机 + 桌面通过；性能预算达标；发布流程可重复。

#### Phase 4：可选能力（按产品决定）

- [ ] ServerQuery（TCP/SSH/WebQuery）。
- [ ] TS6 Streams（屏幕共享信令）。
- [ ] trace/diagnostics/指标。
- [ ] 完整 view 的 UI 集成。
- [ ] fuzz/长期质量基础设施。

> 注意：Phase 4 不应与 Phase 1–3 并行，除非有独立人力；否则会稀释 P0/P1。
>
> **当前实际优先级**：只做“错误修复 + 性能优化”，Phase 3 的命令面/头像图标与 Phase 4 全部暂缓。具体工作单见第 9 节。

---

### 8.10 风险登记

| # | 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|---|
| 1 | `tsclientlib` 固定 rev，安全/功能停滞 | 高 | 高 | **不 fork**；使用公开 API 绕行；持续监控官方上游新版本，只在“纯升级、不改代码”时评估 |
| 2 | Android DNS/hickory 崩溃阻塞升级 | 中 | 高 | 隔离 resolver；保留 0.24 路径；真机回归 |
| 3 | actor 重构破坏绑定/现有调用方 | 中 | 高 | 双轨 API + feature flag + 迁移测试 |
| 4 | 音频设备差异导致采集/播放失败 | 高 | 中 | 设备格式协商 + 重采样 + 真机矩阵 |
| 5 | TS6 协议未文档化，Stream/文件行为不稳定 | 中 | 中 | 能力门禁 + 版本分支 + 降级 |
| 6 | API 破坏导致 Android App 无法升级 | 中 | 高 | semver/迁移指南/ABI 版本 |
| 7 | 真实服务器 CI 不稳定/flaky | 高 | 中 | 录制回放 + 重试 + 隔离环境 |
| 8 | 范围蔓延（ServerQuery/Stream 同时做） | 高 | 高 | MoSCoW + Phase 4 门槛 |
| 9 | 供应链/许可证问题 | 低 | 高 | cargo-deny + SBOM + third-party notices |
| 10 | 安全漏洞（私钥/密码泄漏） | 低 | 高 | zeroize + 脱敏 + 安全评审 |

---

### 8.11 成功指标与 Definition of Done

**成功指标**

- 连接/重连：加入时间、重连成功率、`will_reconnect` 准确率。
- 音频：延迟、ERLE、丢包隐藏、CPU、功耗、crash-free。
- 事件：控制事件丢失 0、语音丢弃率、队列深度。
- 文件：吞吐、续传成功率、上传校验通过率。
- 命令：错误可见率（每个失败都有 `ServerError`）、超时率。
- 质量：CI 矩阵全绿、fuzz 无新崩溃、semver 检查通过。

**DoD（每个任务）**

- [ ] 代码 + 单元/集成测试。
- [ ] 公开 API 文档 + 变更说明。
- [ ] 指标/日志（如适用）。
- [ ] feature flag/回滚路径（行为变更）。
- [ ] CHANGELOG 条目。
- [ ] 绑定层同步（如涉及 ABI/公开类型）。
- [ ] 真实服务器或真机验证（如涉及协议/音频）。

---

### 8.12 明确不做 / 暂缓（当前）

| 项 | 结论 | 条件 |
|---|---|---|
| ServerQuery | 暂缓 | 产品确认需要管理机器人时启动 |
| TS6 Streams | 暂缓 | 产品确认需要屏幕共享时启动 |
| 完整管理命令层 | 按 MoSCoW 裁剪 | 先做 UI 实际用到的 20% |
| 复制 A 的 1536 版本表 | 不做 | 使用依赖层版本枚举 + 配置暴露 |
| 只支持 A 的 identity 格式 | 不做 | 保持 B 格式 + 增加兼容解析 |
| A 的语音恒明文 | 不照搬 | 按服务器设置决策 |
| A 的 license 宽松校验 | 不照搬 | 保留严格校验 |
| WebQuery | 暂缓 | 与 ServerQuery 一起评估 |
| Node/ESM 运行时 | 不做 | B 是 Rust + 原生绑定 |

---

### 8.13 对原 P0/P1/P2 的修正

| 原计划 | 修正 |
|---|---|
| P0-1/2 文件截断/超时 | 保留 P0；增加“流式 API”作为中期目标 |
| P0-3 Bot 事件泵 | 保留 P0，但依赖 actor 骨架；先做最小 actor，再做泵 |
| P0-4 FFI 事件泵 | 保留 P0，与 actor 一起做 |
| P0-5 Java 签名 | 保留 P0；同时补 6 个 native 与 `EchoCanceller.java` |
| P0-6 初始静音 | 降为 P1（产品决策 + 配置开关），不阻塞发布 |
| P0-7 频道 order | 保留 P0（UI 正确性） |
| P0-8 away | 保留 P0（数据正确性，成本低） |
| P0-9 命令 Result | 保留 P0，但作为 Phase 1 的架构任务，而不是逐方法打补丁 |
| P0-10 Connected 两次 | 降为 P1（成本低但不影响核心） |
| P1 全部 | 保留，但把“事件模型/重连/文件续传”提前到 Phase 2 前半 |
| P2-23 view | 提前到 Phase 2（事件模型的自然延伸） |
| P2-24 命令层 | 移到 Phase 3，按 MoSCoW 裁剪 |
| P2-25 头像/图标 | 移到 Phase 3 |
| P2-26/27 ServerQuery/Streams | 移到 Phase 4，需产品门槛 |
| P2-28 版本表 | 只做配置暴露，不做表复制 |
| P2-29 trace | 提前到 Phase 2/3 的可观测性基础 |
| P2-30 fuzz | 提前到 Phase 0 的测试夹具，持续运行 |
| **新增** | **音频 AEC/重采样/jitter/PLC/codec 协商（Phase 1–3）** |
| **新增** | **actor + handle（Phase 1 前置）** |
| **新增** | **依赖层边界与绕行（Phase 0，不 fork）** |
| **新增** | **compatibility/ABI/semver（Phase 0–1）** |
| **新增** | **安全/隐私/供应链（Phase 0–3）** |
| **新增** | **打包/发布/文档（Phase 3）** |

---

## 9. 当前执行计划：错误修复 + 性能优化

> 本节是当前真正要执行的计划。约束：**不改 tsclientlib、不 fork、不打补丁**；只做 B 层能改的错误修复与性能优化。功能对齐（ServerQuery、Streams、管理命令、头像图标、identity 格式）全部暂缓。

### 9.0 范围与目标

> 详细需求、接口、错误语义、验收标准与测试计划见 [spec-error-fixes-performance.md](./spec-error-fixes-performance.md)。

**目标**

1. **错误修复**：不丢数据、不误报成功、不静默失败、不崩溃、状态与服务器一致。
2. **性能优化**：降低事件延迟、CPU、内存、锁竞争、实时音频路径分配，提升弱网/移动端稳定性。

**非目标（本轮不做）**

- ServerQuery、TS6 Streams、完整管理命令层、头像/图标、身份格式对齐、TS6 完整能力。
- 任何需要修改 `tsclientlib/tsproto` 的修复（见 9.3）。

**关键使能点（无需改 D）**

`tsclientlib` 公开了 `OutCommandExt::send_with_result(con)` 与 `StreamItem::MessageResult(MessageHandle, Result)`，可以在 B 层实现命令结果关联：

```rust
// B 层伪代码
let handle = cmd.send_with_result(con)?;      // MessageHandle(return_code)
let (tx, rx) = oneshot::channel();
pending.insert(handle.0, tx);
// process_events 中：
StreamItem::MessageResult(handle, result) => {
    if let Some(tx) = pending.remove(&handle.0) { let _ = tx.send(result); }
}
// 调用方：tokio::time::timeout(config.command_timeout, rx).await
```

这让“命令失败可感知”“超时”“错误码”都能在 B 层完成，不需要改 D。

---

### 9.1 错误修复清单

#### 9.1.1 P0（数据损坏 / 功能不可用 / 崩溃）

| ID | 问题 | 证据 | 影响 | 修复方案 | 验收 |
|---|---|---|---|---|---|
| C-01 | 文件下载超过 10 MB 被静默截断，仍报成功 | `crates/tslib-core/src/client.rs:565-606` | 交付损坏文件 | 去掉 `min(10MB)`；加 `max_download_size: Option<u64>`，超限报 `FileTooLarge`；中期改为流式 `tokio::io::copy` | 20 MB 文件哈希一致；超限有明确错误 |
| C-02 | Bot 从不驱动事件流，收不到消息 | `crates/tslib-bot/src/bot.rs:118-203` | Bot 命令全部无效 | 循环内调用 `client.wait_events(50)`；处理完事件再 `try_recv` 回复；避免 `select!` 取消 `wait_events` 导致丢事件 | 发 `!ping` 有回复 |
| C-03 | FFI 无 runtime/事件泵，连接不推进 | `crates/tslib-ffi/src/lib.rs:61-75,196-246` | C 绑定不可用 | 专用线程 + current_thread runtime + `LocalSet` 持有 `Client`；提供 `tslib_client_poll_events` 或回调队列 | C 示例能连接并收到事件 |
| C-04 | Java `nativeMoveToChannel` 参数个数不匹配 | `java/.../Client.java:214,356`；`crates/tslib-jni/src/client.rs:587-602` | `UnsatisfiedLinkError` | Java 方法增加 `String password` 或 Rust 去掉参数；同步补 6 个缺失 native | Java 调用成功 |
| C-05 | `disconnect()` 后仍保留 connection，发送方法只判断 `connection.is_some()` | `client.rs:1459-1487,1175-1451` | 断开后命令可能静默发出/失败 | 所有发送入口统一检查 `self.state.is_connected()`；`disconnect` 设 `Disconnecting` 并拒绝新命令 | 断开后调用返回 `NotConnected` |
| C-06 | `channel_order` 被截断为 `i32`，树按数值排序 | `client.rs:1635`；`tslib-channel/src/tree.rs:195-205` | 频道顺序错乱/溢出 | `Channel.order: u64`；按前驱链还原顺序；加环检测 | 与官方客户端顺序一致 |
| C-07 | BBCode HTML 渲染不转义文本/属性 | `crates/tslib-chat/src/bbcode.rs:36-74` | 若在 WebView 渲染可 XSS | 文本与属性值 HTML 转义；URL 仅允许 http/https；颜色/尺寸白名单 | XSS 测试用例不执行脚本 |
| C-08 | `wait_connected` 丢弃连接期间的非 `BookEvents` | `client.rs:384-391` | 连接瞬间的消息/音频丢失 | 把非 BookEvents 事件放入 `pending_events`，在 `Connected` 后补发 | 连接期间发的消息不丢 |
| C-09 | 文件命令 `cpw`/`tcpw` 固定空串 | `client.rs:1390,1422,1433-1435,1447` | 密码频道文件操作必失败 | 文件 API 增加 `channel_password`；用 `encode_password` 编码后透传 | 密码频道可列/传/删 |
| C-10 | 音频解码忽略 codec，一律 Opus | `crates/tslib-audio/src/manager.rs:137-142` | 非 Opus 频道解码出噪声 | 按 `AudioCodec` 分派；非 Opus 返回 `UnsupportedCodec` 并静音，不喂给 Opus 解码器 | Speex 频道不再输出噪声 |
| C-11 | `send_audio`/whisper 无大小与目标校验 | `client.rs:1302-1354` | 超大包/超量目标被 D 截断，产生损坏包 | 校验 payload 上限；whisper client ≤32、channel ≤255、至少 1 个目标 | 非法输入返回错误，不发送 |
| C-12 | `ClientConfig` 的 `channel_password/hardware_id/connect_timeout` 未接线 | `config.rs:19-38,110-139`；`client.rs:379` | 配置无效、慢网必失败 | 调用 `ConnectOptions::channel_password/hardware_id`；`wait_connected` 使用 `connect_timeout` | 配置生效；慢网可连 |
| C-13 | `move_to_channel` 在服务器确认前乐观写 `channel_id` | `client.rs:1218` | 失败后本地频道错 | 用 `send_with_result` 等成功再更新，或只在 `BookEvents` 更新 | 失败后 `channel_id()` 不变 |
| C-14 | 重连后订阅/静音/状态不恢复 | `client.rs:412-413,438-441,551-557` | 重连后看不到人/状态不一致 | 检测重连完成，重发 `channelsubscribeall`、`sync_state`、`set_input_muted` | 重连后订阅与静音一致 |
| C-15 | 文件列表/权限查询使用单槽 pending，并发互相覆盖 | `client.rs:105-108,1393,1413` | 并发请求丢结果 | 改为按 request/return_code 或按 channel id 的 map | 并发两个请求都成功 |

#### 9.1.2 P1（状态错误 / 事件缺失 / 绑定缺陷）

| ID | 问题 | 证据 | 影响 | 修复方案 | 验收 |
|---|---|---|---|---|---|
| C-16 | `Connected` 发两次；`ConnectionStateChanged` 从不发射 | `client.rs:186,426`；`connection.rs:175-185` | 事件序列错误 | 只在 `wait_connected` 发一次；删除/接线死模块 | 事件计数正确 |
| C-17 | 7 个事件枚举从不发射（kick/ban/group/ChannelJoined） | `events.rs:36-96`；`client.rs` 无生产者 | 功能缺失 | 从 `notifyclientleftview`/`notifyclientupdated` 映射；自身移动时发 `ChannelJoined` | 每个枚举有测试 |
| C-18 | D 提供的 User/ServerInfo 字段被丢弃 | `client.rs:1590-1597,852-857`；D `structs.rs` | 无上线时间/描述/国家/server UID/banner | 映射 `optional_data/connection_data/description/country_code`；用 `get_server_key()` 算 UID；映射 hostbanner | 字段有值 |
| C-19 | `maxclients` 的 Unlimited/Inherited 都变 -1 | `client.rs:1612-1624` | 编辑/UI 语义错误 | 保留 `MaxClients` 枚举 | 三种状态可区分 |
| C-20 | 权限缺失/查询失败默认“允许” | `client.rs:988,1702-1705` | UI 显示实际不可用的按钮 | 缺失视为未知/不允许；失败返回 `permissions_known=false` | 无权限时不显示操作 |
| C-21 | JNI `FileListReceived` 手拼 JSON；`JNI_OnLoad` 用 Trace | `jni/types.rs:360-371`；`jni/lib.rs:15-30` | 文件名含控制字符时坏；日志泄漏/性能 | 用 `serde_json`；发布版日志级别降到 Info/Warn，消息内容脱敏 | 特殊文件名可解析；日志无正文 |
| C-22 | Python `block_on` 持 GIL；`ChatHistory` 无 `add`；`server_info` 用缓存 | `python/client.rs:80-83,141-143`；`python/message.rs` | 阻塞/历史为空/计数缺失 | 注意 `Client` 非 `Send`，`allow_threads` 不能直接捕获；需专用线程/actor 或接受 GIL 阻塞并文档化；补 `ChatHistory.add`；调用实时 `server_info()` | Python 测试通过，阻塞行为有说明 |
| C-23 | Bot 丢弃 `BotConfig` 的 password/channel/channel_password；owner-only 永远失败；插件命令未注册 | `bot.rs:58-71,167`；`plugin.rs:109-113` | Bot 功能残缺 | 透传配置；从事件取 `sender_uid`；注册插件命令 | owner 命令与插件命令可用 |
| C-24 | `ChannelTree` 重复添加/重复根/无环检测/删父留孤 | `tslib-channel/src/tree.rs:38-100,135-149` | 树结构损坏/死循环 | 去重、重挂、环检测、删除子树 | 树测试通过 |
| C-25 | `joinable_channels` 注释说“未满”，代码只排除密码/spacer | `manager.rs:90-99` | 可能加入已满频道 | 用 state 统计成员数与 `max_clients` 比较 | 满员频道被排除 |
| C-26 | 上传完成未校验服务器落盘大小 | `client.rs:607-641` | 可能报成功但文件不存在 | 上传后用 `ftgetfileinfo` 或 `ftgetfilelist` 校验 | 校验失败报错 |
| C-27 | `sync_state` 清空重建 users/channels | `client.rs:843-895` | UI 闪烁、引用失效、分配抖动 | 增量 diff；保留本地字段 | 同步不丢本地状态 |
| C-28 | `User.is_talking` 恒 false | `client.rs:1576` | UI 说话状态依赖额外事件 | 用 `talking_users` 或 `TalkStatus*` 更新 | 说话时字段为 true |

#### 9.1.3 P2（安全/文档/清理）

| ID | 问题 | 证据 | 修复 |
|---|---|---|---|
| C-29 | `ChannelJoined` 等事件在绑定层有转换但无生产者 | 同 C-17 | 与 C-17 一起修 |
| C-30 | FFI 声明“thread-safe”与 Rust 注释矛盾 | `ffi/lib.rs:14-17`；`cbindgen.toml` | 统一文档；明确句柄线程模型 |
| C-31 | README 的“35+ 事件/242 测试/TS3/6”过期 | `README.md:15-31` | 按实际更新 |
| C-32 | 死代码/死配置 | `connection.rs`、`AudioConfig` 的 AGC/NS/AEC、`rubato` | 接线或删除；不要保留“看起来可用”的开关 |

---

### 9.2 性能优化清单

#### 9.2.1 事件与状态

| ID | 问题 | 证据 | 优化 | 预期收益 |
|---|---|---|---|---|
| P-01 | `process_events` 每项最多等 10 ms，无批量上限；持续音频时可能长时间不返回 | `client.rs:211-232` | 改用 `wait_events` 的事件驱动路径；单批上限 + 合并窗口；避免每项 10 ms | 事件延迟降低，CPU 空转减少 |
| P-02 | `Event` 深拷贝：`event.clone()` 给 broadcast、handler、返回值；音频 `Vec<u8>` 复制 | `client.rs:504-548,254-256,318-320` | `Arc<Event>` 或 `Arc<[u8]>`；语音用共享切片；handler 只读引用 | 音频/事件路径分配显著下降 |
| P-03 | `check_talk_timeouts` 每次扫描全部说话用户 | `client.rs:338-363` | 最小堆/时间轮；按到期时间弹出 | O(n) → O(log n)，空闲时零扫描 |
| P-04 | `users()`/`channels()` 每次克隆全部；`sync_state` 清空重建 | `client.rs:1083-1099,843-895` | 缓存快照 + revision；增量更新；返回 `Arc<[User]>` 或迭代器 | UI 轮询 CPU/分配下降 |
| P-05 | 热路径 `info!` 日志（每条 message、每个文件项、权限 map） | `client.rs:560,902,950,1727` | 降到 `debug!/trace!`；限频 | 生产日志开销下降 |
| P-06 | `broadcast` 容量 256，慢消费者丢控制事件且无计数 | `client.rs:127` | 每消费者队列或 `broadcast` + `Lagged` 处理；加 dropped 计数 | 可观测、可控 |
| P-07 | `add_handler` 串行 await，慢 handler 阻塞泵 | `client.rs:1019-1023` | 有界队列 + 独立消费；或明确文档要求 handler 快速返回 | 防止单 handler 拖垮连接 |

#### 9.2.2 音频实时路径（优先，移动端收益最大）

| ID | 问题 | 证据 | 优化 | 预期收益 |
|---|---|---|---|---|
| P-08 | 采集回调每帧分配 `Vec<i16>`、加锁 `Mutex<Vec>`、`drain` 拷贝 | `crates/tslib-audio/src/capture.rs:149-195` | 预分配环形缓冲/SPSC；回调内不分配；用固定帧数组 | 消除 GC/分配抖动，降低丢帧 |
| P-09 | 播放回调每次分配 `pcm_buffer`；mixer 每次分配 `temp` | `playback.rs:143`；`mixer.rs:48` | 预分配复用缓冲区；mixer 用 `&mut` 复用 | 消除回调内分配，减少爆音 |
| P-10 | 播放/混音回调持 `Mutex`，可能与推送线程竞争 | `playback.rs:145-147` | 无锁环形缓冲/双缓冲；临界区最小化 | 降低 underrun |
| P-11 | 每包 `RwLock<HashMap>` 写锁 + 新建 `samples` Vec | `manager.rs:144-158` | 每用户 actor 或分片锁；复用输出缓冲 | 降低解码延迟/锁竞争 |
| P-12 | 解码器只增不减（`remove_user` 存在但需确认调用） | `manager.rs:22,195-198` | 用户离开/超时清理；上限淘汰 | 长会话内存稳定 |
| P-13 | 无 jitter buffer，播放环形缓冲覆盖未读数据 | `playback.rs:265-291` | 按 `seq` 的**固定 60 ms** bounded jitter buffer（3 帧）；迟到即丢，不随网络增长；丢帧计数 | 弱网连续性提升且延迟可预测 |
| P-14 | 无重采样，设备非 48 kHz 直接失败 | `AudioConfig` 48 kHz；`rubato` 未用 | 采集/播放格式协商 + rubato 重采样 | 兼容更多设备 |
| P-15 | PLC 未接线，丢包直接静音/噪声 | `handle_packet_loss` 无调用方 | 根据 `seq` gap 调 `decode_plc`；限制连续 PLC 次数 | 丢包听感提升 |
| P-16 | `playback_buffer_ms` 60 ms 实际按 ×4 分配（约 240 ms） | `playback.rs:47` | 明确缓冲语义；降低默认延迟或可配置 | 端到端延迟下降 |
| P-17 | 采集 `try_send` 满时静默丢帧 | `capture.rs:191-203` | 加 dropped 计数/指标；必要时降采样 | 可观测 |

#### 9.2.3 绑定与平台

| ID | 问题 | 证据 | 优化 | 预期收益 |
|---|---|---|---|---|
| P-18 | Python `block_on` 持 GIL | `python/client.rs:80-83` | 专用线程 + 通道（`Client` 非 `Send`，不能直接用 `allow_threads`）；或文档化阻塞 | 不阻塞其他 Python 线程 |
| P-19 | JNI `nativeDestroy` 自旋 2 s；`nativeDisconnect` sleep 500 ms | `jni/client.rs:105-132,317-321` | 条件变量/oneshot 等待；disconnect 异步化 | UI 卡顿下降 |
| P-20 | FFI 无事件泵 | `ffi/lib.rs` | 专用线程 + 队列（与 C-03 一起） | FFI 可用 |
| P-21 | JNI 每次事件构建大量 Java 对象/字符串 | `jni/types.rs` | 复用/批量；必要时缓存字段 id | 事件吞吐提升 |
| P-22 | 绑定层无 benchmark | — | 加 JNI/Python smoke + 吞吐测试 | 可持续回归 |

---

### 9.3 依赖层限制（不改 D，只能接受/绕行）

| 项 | 限制 | 处理 |
|---|---|---|
| 语音加密模式 | D 固定 `voice_encryption=true` | 接受“始终加密”；文档记录；不实现“跟随服务器” |
| 接收乱序 ACK/空洞 | D 先 ACK 未处理包，无 hole timeout | B 加 watchdog：长时间无事件/无 pong 主动重连；不能根治 |
| TS6 文件时间 | D 按秒解析且不保留原始值 | 接受；文档标注 TS6 文件时间不可用 |
| Init0/握手 step/puzzle 上限 | D 固定 | 接受；不处理 |
| 文件 `proto=1` | D 硬编码 | 暂接受；若实测 IPv4-only 有问题，再评估自研传输 |
| `capability_extensions` 状态 | book 不保存；`InServerInfo` 未声明属性 | 大概率拿不到；尝试 `serverinfo` 后仍失败则记录为限制 |
| `clientek` 判定 | D 按首包 id/前缀 | 接受 |
| `away` 独立标志 | book 只有 `away_message` | 接受启发式；文档标注 |

---

### 9.4 测量方法与基线（先测再改）

**必须建立基线，否则“性能优化”无法验收。**

1. **事件路径**
   - 控制事件/语音事件的投递延迟 p50/p99。
   - 每秒可处理事件数；`process_events` 单次耗时。
   - 事件队列深度、丢弃数。
2. **音频路径**
   - 采集回调分配次数/帧、锁等待时间。
   - 播放回调分配次数、underrun 次数。
   - 端到端延迟（采集→发送→接收→播放）。
   - 5% 丢包下的 PLC/jitter 表现。
3. **状态路径**
   - `users()`/`channels()` 每次调用耗时/分配。
   - `sync_state` 耗时。
   - 1000 用户/200 频道规模下的内存与 CPU。
4. **绑定**
   - Python `process_events` 吞吐与 GIL 阻塞。
   - JNI 事件转换吞吐、destroy/disconnect 阻塞时间。
5. **工具**
   - Rust：`criterion`、`dhat`/heaptrack、`tokio-console`。
   - Android：Perfetto/Macrobenchmark、`dumpsys media.audio_flinger`。
   - 基线数据写入 `docs/perf-baseline.md`。

---

### 9.5 执行顺序（按批次）

#### Batch 1：P0 数据正确性（第 1 周）

- C-01 文件截断/超时
- C-05 断开后状态
- C-06 频道顺序
- C-07 BBCode 转义
- C-08 连接期事件丢失
- C-11 语音/whisper 校验

**退出**：文件 >10 MB 一致；频道顺序正确；XSS 用例通过；连接期消息不丢。

#### Batch 2：P0 可用性/绑定（第 2 周）

- C-02 Bot 事件泵
- C-03 FFI 事件泵
- C-04 Java 签名
- C-09 文件密码
- C-10 codec 分派
- C-12 配置接线
- C-13 移动状态
- C-14 重连恢复
- C-15 并发 pending

**退出**：Bot/FFI/Java smoke 通过；密码频道文件可用；重连后状态一致。

#### Batch 3：P1 正确性（第 3–4 周）

- C-16 ~ C-28（事件、字段、权限、树、Bot、JNI/Python）
- 同时建立 `docs/perf-baseline.md`

**退出**：P1 验收全绿；事件/字段测试覆盖。

#### Batch 4：性能优化（第 5–7 周）

- P-01 ~ P-07 事件/状态
- P-08 ~ P-17 音频实时路径
- P-18 ~ P-22 绑定
- 每项改前/改后对比基准

**退出**：目标指标达成，且无功能回归。

#### Batch 5：回归与文档（第 8 周）

- Android 真机 + 桌面回归
- 更新协议/限制文档、README
- 记录 D 层限制

---

### 9.6 回归测试清单

**错误修复**

- [ ] 20 MB 文件上传/下载，SHA-256 一致；超限有明确错误。
- [ ] 密码频道列目录/上传/下载/删除。
- [ ] 并发两个文件列表/权限查询，都拿到结果。
- [ ] 断开后调用发送 API 返回 `NotConnected`。
- [ ] 频道树顺序与官方客户端一致；含根/子/移动/删除。
- [ ] BBCode HTML 不执行 `<script>`；`javascript:` URL 被拒绝。
- [ ] 连接期间发送的文本消息在 `Connected` 后可见。
- [ ] 说话时 `User.is_talking` 为 true，停止后为 false。
- [ ] 重连后订阅、静音、频道 ID 正确。
- [ ] 非法 whisper 目标/超大语音帧返回错误。
- [ ] 非 Opus 频道不输出噪声。
- [ ] Bot `!ping`、owner-only、插件命令。
- [ ] FFI/C 示例、Java `moveToChannel`、Python `process_events`。

**性能**

- [ ] 事件延迟 p99 不超过基线；CPU 不升高。
- [ ] 采集/播放回调 0 分配（用 dhat/heaptrack 验证）。
- [ ] 5% 丢包下音频连续性优于基线。
- [ ] 1000 用户/200 频道的 `users()`/`channels()`/`sync_state` 耗时不高于基线。
- [ ] Android 真机 30 分钟通话：CPU、内存、underrun、crash 达标。

---

### 9.7 完成标准

- P0 清零；P1 全部有测试覆盖或明确“接受限制”。
- 性能基线有改前/改后对比；关键路径无回归。
- D 层限制在 `docs/protocol-behavior.md` 有记录（语音加密、TS6 时间、接收窗口、文件 proto）。
- 绑定层 smoke 测试进 CI。
- README/文档不再夸大能力。
- **公开 API 变更按 `docs/api-changes/README.md` 记录，产出 `docs/api-changes/v<version>.md`。**

---

## 附录：与 A 的差距速查（精简）

| 能力 | A | B | 备注 |
|---|---|---|---|
| TS3 语音客户端 | ✅ | ✅ | 主干一致 |
| TS6 完整能力 | ✅ | ❌ | capabilities/UID SHA-256/命令 |
| 屏幕共享信令 | ✅ | ❌ | `A/src/stream` |
| ServerQuery | ✅ | ❌ | TCP/SSH/WebQuery |
| 命令覆盖 | ~162 | ~10 | 管理能力差距最大 |
| 命令结果/错误 | ✅ | ❌ | fire-and-forget |
| 文件传输 | 流式/续传/停止 | 内存/10 MB/无续传 | P0 |
| 事件模型 | typed+raw+迭代器 | broadcast | P1 |
| 重连/限流 | 可配置 | 死配置/无 | P1 |
| 身份互通 | `base64:offset` | `offsetV...` | 需拍板 |
| 音频处理 | ❌ | ✅ | B 优势 |
| 原生绑定 | ❌ | ✅ | B 优势 |
| BBCode/Bot/频道树 | ❌ | ✅ | B 优势 |

---

> 本文只记录建议与证据，不构成对现有行为的修改承诺。**当前执行以第 9 节为准**（错误修复 + 性能优化，不改 tsclientlib、不 fork）；第 8 节是完整评审背景，第 3 节仅作差距分组参考；产品范围取舍见第 5 节。
