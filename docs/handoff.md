# 交接 / 当前进度（Handoff）

> 最后更新：2026-11-05  
> 当前 HEAD：`fa83b5c feat(core): 补充 C6 字段更新、图标 id 与打包脚本`  
> 工作区：干净  
> 施工清单：`docs/construction-todo.md`（C1–C6 已完成，C7 验证遗留）  
> 持久规范：`AGENTS.md`；需求：`docs/spec-error-fixes-performance.md`；API 差异：`docs/api-changes/v0.2.0.md`

---

## 1. 本轮已完成

### 错误修复

| 提交 | 内容 |
|---|---|
| `d08b869` | BBCode HTML 转义、危险 URL/颜色/字号拦截 |
| `b0d4174` | 语音帧大小与 whisper 目标数量校验 |
| `b383c16` | 断开后所有发送 API 返回 `NotConnected` |
| `8a390e0` | 文件下载禁止 10 MB 截断；chunk 读写 + idle timeout |
| `c5f2f7d` | 握手期间非 `BookEvents` 缓存，不再丢消息 |
| `244038c` | `Channel.order` 改 `u64`，按前驱链还原频道顺序 |
| `742b37c` | `Connected` 只发一次 |
| `d9346eb` | `channel_password`/`hardware_id`/`connect_timeout` 接线 |
| `76b69cf` | 非 Opus codec 返回 `UnsupportedCodec` |
| `ac121a2` | Bot 驱动 `wait_events`；owner-only、插件命令、BotConfig 接线 |
| `8193b9c` | Java `moveToChannel` 签名对齐并支持频道密码 |
| `3030d72` | 文件操作支持频道密码（6 个 `*_with_password`） |
| `3b9087b` | 文件列表/权限查询并发不互相覆盖 |
| `12447fa` | 移动频道不再乐观更新 `channel_id` |
| `b74e203` | 重连后恢复订阅/静音/状态，新增 `Reconnected` |
| `8a94180` | FFI 增加 runtime、`pump`、回调 |
| `7b26af4` | User/ServerInfo 可选字段映射；server UID |
| `640aa6b` | kick/ban/`ChannelJoined` 事件 |
| `47a1814` | 权限缺失不再默认允许文件操作 |
| `5d48793` | 热路径日志降级 |
| `17b4dee` | `User.is_talking` 维护 |
| `988f1eb` | Java 缺失 native 补齐 + `AudioSink` |
| `5f00eaa` | Java `EchoCanceller` |
| `401bfed` | FR-AU-06 解码器清理：`UserLeft` 事件桥、5 分钟空闲淘汰、512 LRU 上限 |
| `d1c48f1` | PR-03 说话超时改为有序截止时间检查 |
| `2290371` | PR-08/09 采集/播放/混音回调复用缓冲 |
| `d12b53a` | PR-02 `AudioReceived.data` 改 `Arc<[u8]>` |
| `2e0e4cd` | FR-SC-02 `ClientConfig`/Builder `Debug` 密码脱敏 |
| `1da8cc6` | FR-FS-06 上传后 `ftgetfileinfo` 校验落盘大小 |
| `7a42355` | FR-BD-04 Python 历史写入/实时 server info/事件负载/GIL 文档 |
| `76d4525` | PR-10 音频回调 `try_lock`，实时线程不阻塞 |
| `81f742d` | FR-BD-05 JNI FileList `serde_json`；发布版日志 `Info`；日志不输出正文 |
| `9fa75bf` | FR-ST-02/05 `MaxClientsKind`、`Inherited` 解析、按人数排除满员频道 |
| `d16b46e` | FR-ST-03 收尾：`PermissionHints.known`、Java/Python 绑定 |
| `680402c` | FR-ST-04 删除子树、reparent 环检测、路径/打印防环 |
| `033078b` | PR-13 采集/播放/解码/PLC 可观测计数 |
| `1cf4054` | PR-04（增量）`users_snapshot`/`channels_snapshot` 共享快照 |
| `8483907` | `AudioReceived` 暴露 `seq`，供下游检测丢帧/PLC |
| `384460b` | FR-AU-03/04 固定 60 ms jitter buffer、迟到丢弃、gap PLC |
| `12d5ef2` | C1 正确性收尾：配置版本/初始静音/watchdog、TextMessage.target_id、User 时间字段、users/channels Arc |
| `9168f09` | C2 actor：ActorHandle/EventStream、命令结果、Notification、Python/FFI/JNI 适配 |
| `337e990` | C3 路径式文件传输、ftstop/ftlist/ftgetinfo、路径/文件事件 |
| `a0df054` | C4 重采样/设备协商、AEC、AGC/NS、解码器缓冲复用、播放缓冲 40 ms |
| `5ce6f28` | C5 泵统计、队列深度、基准与 semver CI |
| `b3df50f` | C6 类型 ID、视图/revision、管理命令、raw/组 whisper、ServerQuery、打包/fuzz |
| `fa83b5c` | C6 补充 FieldUpdate、CRC32 图标 id、打包脚本 |

### 验证

- `tslib-core` 92 passed；`tslib-bot` 30；`tslib-audio` 56；`tslib-channel` 47；`tslib-chat` 116；`tslib-serverquery` 3 + 1 doc。
- 合并测试用 `-j 1` 通过（默认并行偶发 rustc 崩溃）。
- `cargo check -p tslib-ffi -p tslib-python -p test-suite` 通过。
- Host JNI：`cargo check/build -p tslib-jni` 通过；`dumpbin` 确认符号。
- Java：JDK 21 `javac` 全部通过；`javap -s` 核对 descriptor。
- Android arm64 / x86_64：`cargo ndk -t <abi> check -p tslib-jni --features vendored-openssl` 均通过（`CARGO_EXIT=0`）。
- JNI 单元测试 2 passed；Java `javac`（JDK 21）通过，`Channel` 构造描述符已核对。
- PR-08/09 为稳态零逐帧分配：缓冲池空或首次回调仍会分配；尚未用 dhat 建立分配基线。

---

## 2. 当前设计决策（已确认）

| 项 | 决定 |
|---|---|
| 依赖层 | 不修改 `tsclientlib/tsproto`、不 fork、不打补丁 |
| jitter buffer | **固定 60 ms（3 帧）**，迟到即丢，不随网络增长；`JitterBuffer` + `process_incoming_with_seq`/`pump_jitter` |
| 解码器池 | 空闲 **5 分钟**淘汰；上限 **512**，LRU；`UserLeft` 立即清理 |
| 音频事件负载 | `Arc<[u8]>`，广播与返回事件共享，不再深拷贝 |
| 上传完成判定 | 以 `ftgetfileinfo` 落盘大小为准；2568 无权限仅 warning |
| 敏感信息 | `ClientConfig`/Builder 的 `Debug` 输出 `<redacted>` |
| 频道容量 | `MaxClientsKind` 区分 Unlimited/Inherited/Limited/Unknown；`Inherited` 沿父链解析 |
| 权限未知 | 查询失败 `known=false`、`bits=0`，不再回退 `JOIN` |
| 频道删除 | `remove_channel` 删除整棵子树，与服务器一致 |
| 用户/频道读取 | `users()`/`channels()` 返回共享 `Arc<[T]>`；事件泵/状态变更后重建 |
| 音频事件 | `AudioReceived` 携带 `seq`；`process_incoming_with_seq` + `pump_jitter` 接入固定 jitter/PLC |
| `max_memory_download` | 默认 **64 MiB**；超出报 `FileTransferFailed`，不截断 |
| 初始静音 | `ClientConfig.initial_input_muted`，默认 **true** |
| 组 whisper | `send_audio_to_group` 构造 `C2SWhisperNew`（target/whisper_type 由协议/服务器决定） |
| 命令结果等待 | `ActorHandle::execute` + 命令便捷方法，带超时；旧 fire-and-forget API 保留 |
| Go/No-Go | 高风险项先设计/拍板，不默认丢弃 |

---

## 3. 施工状态

完整施工清单见 [`docs/construction-todo.md`](construction-todo.md)。

- **C1–C6：已完成**（正确性收尾、actor/命令/事件、路径式文件、音频硬化、性能/CI、P2 能力）。
- **C7：验证遗留**

### C7 剩余

- 真实 TS3/TS6 服务器集成测试（文件续传、上传校验、权限、语音、重连）。
- Android 真机音频/AEC/功耗/30 分钟通话；端到端 p95 延迟。
- `docs/perf-baseline.md` 补真实连接 p99 与设备回调分配数据。
- Java JUnit/仪器测试。
- `scripts/package.sh` 在 CI/发布机上的实际产物验证。

### 已知实现边界（非阻塞）

- C4-7 时钟漂移：已提供 `PlaybackStats.underruns` 观测与 `restart_*` 钩子，未做自适应重采样。
- C6-9 ServerQuery：实现 TCP 文本协议；SSH/WebQuery 传输未实现。
- C6-10 TS6 Streams：仅可行性评估（`docs/streams-feasibility.md`），未实现。
- 采集/播放设备回调的 0 分配仅由 mixer/事件基准与代码审查支持，待真机 dhat/Perfetto 复核。

---

## 4. 依赖层限制（接受）

| 限制 | 处理 |
|---|---|
| 语音加密固定 `voice_encryption=true` | 接受“始终加密” |
| 接收窗口 ACK/无空洞超时 | 只能 watchdog 缓解 |
| TS6 文件时间 | 依赖层按秒解析且不保留原值，接受 |
| 文件 `proto=1` | 暂接受 |
| `capability_extensions` | 拿不到，记录 |
| `clientek` 判定 | 接受 |
| `away` 独立 bool | 只有 `away_message`，启发式 |

---

## 5. 续接步骤

1. 读 `AGENTS.md` 的环境/踩坑。
2. 读 `docs/spec-error-fixes-performance.md` 第 9 节与 `docs/api-changes/v0.2.0.md`。
3. 从 P1 低风险项挑一个：
   - 先写/改测试；
   - 只格式化改动文件（**不要 `cargo fmt --all`**）；
   - `cargo test -j 1 ...`；
   - 更新 `docs/api-changes/v0.2.0.md` + `CHANGELOG.md`；
   - 一个修复一个 commit。
4. 涉及 JNI/Java 时按 `AGENTS.md` 的 MSVC/JDK/NDK 命令验证。
5. 涉及 Android 时用 `--features vendored-openssl` + `PERL5LIB` + Ninja toolchain wrapper。
