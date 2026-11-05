# Changelog

本项目在 `0.x` 期间遵循“minor 可包含破坏性变更、patch 不破坏”的原则。  
公开 API 的旧/新对照与迁移片段见 [`docs/api-changes/`](docs/api-changes/README.md)。

## [0.2.0] - 未发布

### Added

- `FileTransferConfig`（默认 64 MiB 内存上限、30 s 空闲超时、64 KiB 分块）及 `Client::set_file_transfer_config` / `Client::file_transfer_config`。
- `FileTransferConfig::unlimited()`，用于显式关闭内存上限与空闲超时。
- `Error::InvalidArgument`，用于调用方参数校验失败。
- `AudioError::UnsupportedCodec`，用于非 Opus 频道。
- Java `Client.moveToChannel(long, String)` 重载，支持频道密码。

### Changed

- `Channel.order` 由 `i32` 改为 `u64`，语义明确为“前驱频道 id”；Java `Channel.order` 同步为 `long`，Python 暴露为 `u64`。
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

### Security

- BBCode HTML 渲染对标签外文本和属性做 HTML 转义，拒绝 `javascript:`/`data:` 等显式 scheme，非法颜色/字号降级为纯文本。
- 修复 `<script>`、`javascript:` 链接、`data:` 图片等 XSS 向量。

### Migration

- 完整迁移说明：[`docs/api-changes/v0.2.0.md`](docs/api-changes/v0.2.0.md)
- 破坏性项：`Channel.order` 类型、Java/Python 绑定字段类型、`Error` 新增变体、文件超限/超时行为。
