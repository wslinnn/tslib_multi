# Changelog

本项目在 `0.x` 期间遵循“minor 可包含破坏性变更、patch 不破坏”的原则。  
公开 API 的旧/新对照与迁移片段见 [`docs/api-changes/`](docs/api-changes/README.md)。

## [0.2.0] - 未发布

### Added

- `FileTransferConfig`（默认 64 MiB 内存上限、30 s 空闲超时、64 KiB 分块）及 `Client::set_file_transfer_config` / `Client::file_transfer_config`。
- `FileTransferConfig::unlimited()`，用于显式关闭内存上限与空闲超时。
- `Error::InvalidArgument`，用于调用方参数校验失败。
- `Error::FileTransferCorrupted`，用于上传后服务器落盘大小与本地不一致。
- `AudioError::UnsupportedCodec`，用于非 Opus 频道。
- Java `Client.moveToChannel(long, String)` 重载，支持频道密码。
- `Client` 新增 `download_file_with_password`、`upload_file_with_password`、`list_files_with_password`、`delete_file_with_password`、`rename_file_with_password`、`create_directory_with_password`。
- `Event::Reconnected`，用于区分首次连接与重连恢复。
- FFI 新增 `tslib_client_wait_connected`、`tslib_client_pump`、文本/状态回调注册。
- Java `Client` 新增 `waitEvents/sendPoke/getNetworkStats/updateServerVariables/setWhisperTargets/setAudioSink` 与 `AudioSink` 接口。
- Java `Identity` 新增 `exportTeamSpeakIni`；`OpusCodec` 新增 `decodePlc/setBitrate/setFec/setExpectedPacketLoss`。
- 新增 Java `EchoCanceller`（AEC3）。
- `DecoderPoolConfig`（默认 5 分钟空闲淘汰、最多 512 个解码器）及 `AudioConfig.decoder_pool`。
- `AudioManager::handle_event` / `handle_events`，把 `UserLeft` 映射为解码器与混音缓冲清理。
- `AudioManager::evict_idle_decoders`、`AudioManager::decoder_count`，用于主动回收与诊断。
- `SampleBufferPool` 与 `CaptureDevice::start_with_pool`，在采集回调与编码任务之间复用 PCM 帧缓冲。
- `ChatHistory::add_server_message` / `add_channel_message` / `add_private_message`（Rust 与 Python 同步）。
- Python 事件字典补齐 `audio_received.data`、`file_downloaded.data`、`file_list_received.files`。
- Python `Client.server_info()` 改为读取实时状态，不再返回上次同步的缓存。
- `MaxClientsKind`（`Unlimited`/`Inherited`/`Limited`/`Unknown`）与 `Channel.max_clients_kind`、`max_family_clients_kind`。
- `ChannelTree::effective_max_clients` / `is_full`，沿父链解析 `Inherited` 上限。
- `ChannelManager::joinable_channels_for_users` / `joinable_channels_with_occupancy`，按人数排除满员频道。
- `PermissionHints { bits, known }` 与 `Channel.permission_hints_known`；Java `Channel.permissionHintsKnown`，Python `Channel.permission_hints(_known)`。
- `Event::ChannelPermissionsUpdated` 新增 `known` 字段。
- `AudioStats` / `CaptureStats` / `PlaybackStats` 与 `AudioManager::stats`、`CaptureDevice::stats`、`PlaybackDevice::stats`（采集丢帧、回调争用、播放静音块、解码/PLC 次数、解码器数量）。
- `Client::users_snapshot` / `channels_snapshot`，事件泵之间重复读取共享同一份 `Arc` 快照；旧 `users()`/`channels()` 保持返回 `Vec`。
- `Event::AudioReceived` 新增 `seq`，下游可检测丢帧；Java/Python 事件字典同步暴露 `seq`。
- `JitterBuffer` / `JitterOutput` / `JitterStats` 与 `JitterBufferConfig`（固定 60 ms、20 ms 帧、最多 3 帧 PLC）。
- `AudioManager::process_incoming_with_seq` 与 `pump_jitter`：按序号缓冲、迟到丢弃、gap 触发 PLC；旧 `process_incoming` 保持直通。
- `ClientConfig.version`（`ClientVersion`）、`initial_input_muted`（默认 `true`）、`reconnect_watchdog`（默认 30 s）。
- `Event::TextMessage.target_id`：频道消息为频道 id，私聊为目标 client id。
- `User.created_at` / `last_connected_at`（unix 秒，服务器提供时）。
- `Event::ConnectionStalled`：临时断线超过 watchdog 时发一次，不改变重连状态。
- `ChannelManager::joinable_channels` 标记 `#[deprecated]`，推荐按人数过滤的新方法。
- `ActorHandle` / `ActorEventStream`：客户端在专用线程运行，命令带超时返回结果，事件按消费者分发；控制事件不丢，语音事件 drop-oldest 并计数。
- `Event::Notification { command }`：未专门映射的协议消息不再静默丢弃。
- FFI 新增 `tslib_actor_*`（连接、pump、命令结果、状态、语音丢弃计数）。
- Java 新增 `ActorClient`（命令结果 + `pumpEvents`），旧 `Client` 保持兼容。
- Python `Client` 改为 actor 后端：构造函数完成连接，`process_events` 非阻塞，新增 `wait_events`。

### Changed

- `Channel.order` 由 `i32` 改为 `u64`，语义明确为“前驱频道 id”；Java `Channel.order` 同步为 `long`，Python 暴露为 `u64`。
- `Event::AudioReceived.data` 由 `Vec<u8>` 改为 `Arc<[u8]>`，广播订阅者与返回事件共享同一份音频负载。
- 上传完成事件改为在 `ftgetfileinfo` 校验服务器落盘大小后发出；无浏览权限（2568）记录 warning 后视为成功。
- 频道树按 `channel_order` 前驱链还原兄弟顺序，不再按数值排序；环/缺失前驱按 id 回退。
- `ChannelTree::add_channel` 每次重建结构，重复/乱序插入不再产生重复节点。
- 文件传输改为按 chunk 读写、每个 chunk 重置 idle timeout，移除 5 s/10 s 总超时。
- `wait_connected` 不再丢弃握手期间的非 `BookEvents`，改为缓存到下一次事件泵。
- `disconnect()` 后所有发送 API 统一返回 `NotConnected`。
- `send_audio` 与 `set_whisper_targets` 增加帧大小/目标数量校验。
- `ClientConfig.channel_password`、`hardware_id`、`connect_timeout` 已接线并生效。
- `Connected` 只在状态同步后发送一次，不再重复。
- Bot 每轮驱动 `wait_events`；owner-only 使用发送者 UID；插件命令在加载时注册；`BotConfig` 的密码/频道参数传入客户端。
- `AudioManager` 仅对 Opus Voice/Music（4/5）解码，其他 codec 返回 `UnsupportedCodec`。
- `AudioManager` 按用户维护解码器时间戳：空闲超时自动淘汰，数量超限淘汰最久未用；`process_incoming` 创建解码器失败返回错误而不是 panic。
- 文件列表/权限查询按 `return_code` 关联，支持并发；同一频道/路径重复请求返回 `InvalidArgument`。
- `move_to_channel` 不再发送后立即写 `channel_id`，只在服务器确认后更新。
- 重连后重订阅频道、同步状态、恢复静音，并清空失效的 whisper 目标。
- `User.country/description/idle_time/connected_time` 映射依赖层已有字段。
- `ServerInfo.uid` 按服务器版本计算 SHA-1/SHA-256，banner 字段同步映射。
- 用户离开按 reason 区分为频道踢出、服务器踢出、封禁；自己移动频道发 `ChannelJoined`。
- 文件权限缺失不再默认允许；权限查询失败只保留 join hint。
- `User.is_talking` 随音频与 TalkStatus 维护。
- 逐事件/逐文件/权限计算日志从 info 降到 debug，降低热路径开销。
- 说话超时改为有序截止时间检查：无人说话时零扫描，有更新时为 O(log n)。
- 采集回调不再创建中间 `Vec`，帧缓冲从池中复用；播放与混音的 PCM 临时缓冲跨回调复用，稳态回调不再逐帧分配。
- 采集/播放回调改用 `try_lock`：拿不到锁时丢弃本次采集或输出静音，实时线程不再等待网络/解码线程。
- `Channel::is_full` 优先使用结构化上限；`max_clients` 保留原有 `-1` 语义。
- 权限查询失败不再回退 `JOIN` 提示，改为 `known=false`、`bits=0`，调用方可区分“未知”和“拒绝”。
- `Client::users()` / `channels()` 改为返回 `Arc<[User]>` / `Arc<[Channel]>`，事件泵/状态变更后重建；`users_snapshot` / `channels_snapshot` 作为弃用别名。
- 初始输入静音改为由 `ClientConfig.initial_input_muted` 控制，默认仍为 `true`。
- `auto_reconnect` / `reconnect_delay` / `max_reconnect_attempts` 标记 `#[deprecated]`（重连策略由依赖层负责）。
- `ChannelTree::remove_channel` 删除整棵子树（与服务器行为一致）；新增 `remove_subtree` 返回全部被删频道。
- `ChannelTree::update_channel` 统一重建结构，拒绝把频道挂到自己的后代下；`path_to`/`print_tree` 遇环终止。

### Fixed

- 文件下载超过 10 MiB 时被静默截断且仍报成功；现在超限返回明确的 `FileTransferFailed`。
- 上传 resume offset 超过数据长度时可能静默截断；现在返回 `FileTransferFailed`。
- 握手期间到达的文本/音频事件被丢弃。
- 频道 id 大于 `i32::MAX` 时 `Channel.order` 溢出。
- 断开后仍可能把命令交给残留连接。
- `Connected` 事件重复发送（第一次 server_name 为空）。
- Bot 不驱动事件流导致收不到消息；owner-only 永远失败；插件命令未注册。
- 非 Opus 频道被喂给 Opus 解码器产生噪声。
- Java `nativeMoveToChannel` 参数个数与 Rust 导出不一致，调用会 `UnsatisfiedLinkError`。
- FFI 没有 runtime/事件泵，连接无法被驱动。
- 文件列表/权限查询并发时互相覆盖。
- 重连后订阅、静音和本地状态丢失。
- 长时间多用户会话中，已离开或长期静默用户的 Opus 解码器常驻内存。

### Security

- BBCode HTML 渲染对标签外文本和属性做 HTML 转义，拒绝 `javascript:`/`data:` 等显式 scheme，非法颜色/字号降级为纯文本。
- 修复 `<script>`、`javascript:` 链接、`data:` 图片等 XSS 向量。
- `ClientConfig`、`ClientConfigBuilder` 的 `Debug` 不再输出服务器密码/频道密码，改为 `<redacted>`。
- JNI `FileListReceived.files` 改用 `serde_json` 序列化，文件名中的引号、反斜杠、控制字符不再破坏 JSON。
- JNI 发布构建日志级别固定为 `Info`（debug 构建为 `Trace`）；Rust 侧不再在日志中输出消息正文，只记录长度。

### Migration

- 完整迁移说明：[`docs/api-changes/v0.2.0.md`](docs/api-changes/v0.2.0.md)
- 破坏性项：`Channel.order` 类型、Java/Python 绑定字段类型、`Error` 新增变体、文件超限/超时行为。
