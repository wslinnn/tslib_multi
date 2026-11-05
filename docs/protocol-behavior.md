# 协议行为与限制（实测记录）

> 对应施工项：C7-4；记录 B 侧已知行为与依赖层限制，供下游/测试参考。

## 连接与重连

- `Client` 非 `Send/Sync`；跨线程使用 `ActorHandle`（专用线程 + 命令/事件通道）。
- 临时断线由 tsclientlib 自动重建；B 侧 `reconnect_watchdog`（默认 30 s）只发一次 `ConnectionStalled`，不强制断开。
- 重连完成后重新订阅频道、同步状态、恢复静音、清空 whisper 目标，并发 `Reconnected`。

## 语音

- 只解码 Opus Voice/Music（codec 4/5）；其他 codec 返回 `UnsupportedAudioCodec`，不产生噪声。
- 新接收路径：固定 60 ms jitter、迟到丢弃、gap 最多 3 帧 PLC；旧 `process_incoming` 仍直通。
- 语音加密固定开启（依赖层不暴露 `codec_encryption_mode`）。
- 组 whisper 使用 `send_audio_to_group`（NEWPROTOCOL 包），具体 target/whisper_type 由 TS6 协议/服务器决定。

## 文件

- 默认内存下载上限 64 MiB，超限 `FileTransferFailed`，不静默截断。
- 每 64 KiB chunk 30 s 空闲超时，无总时长限制。
- 路径式下载/上传支持 offset/resume；上传完成后 `ftgetfileinfo` 校验 size，不匹配 `FileTransferCorrupted`。
- 文件 `proto=1` 由依赖层硬编码；TS6 文件时间按秒解析。

## 频道/权限

- `Channel.order` 为前驱频道 id，按链还原顺序。
- `max_clients` 保留 `-1`；`MaxClientsKind` 区分 Unlimited/Inherited/Limited/Unknown，`Inherited` 沿父链解析。
- 权限查询失败 `known=false/bits=0`；无浏览权限（2568）只 warning。
- 删除频道删除整棵子树；reparent 环被拒绝；路径/打印有环保护。

## 已知限制（需要改依赖层）

- 语音加密模式、接收窗口 ACK/空洞根治、TS6 文件原始时间、握手 puzzle 上限、`proto=1`、`capability_extensions`、`clientek` 判定。
- `Client` 的 `!Send` 只能通过 actor 规避，不能根治。
