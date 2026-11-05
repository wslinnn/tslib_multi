# 交接 / 当前进度（Handoff）

> 最后更新：2026-11-05  
> 当前 HEAD：`7a42355 feat(python): 补齐历史写入、实时服务器信息与事件负载`  
> 工作区：干净  
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

### 验证

- `tslib-core` 77 passed；`tslib-bot` 30；`tslib-audio` 33；`tslib-channel` 39；`tslib-chat` 116。
- 合并测试用 `-j 1` 通过（默认并行偶发 rustc 崩溃）。
- `cargo check -p tslib-ffi -p tslib-python -p test-suite` 通过。
- Host JNI：`cargo check/build -p tslib-jni` 通过；`dumpbin` 确认符号。
- Java：JDK 21 `javac` 全部通过；`javap -s` 核对 descriptor。
- Android arm64：`cargo ndk -t arm64-v8a check -p tslib-jni --features vendored-openssl` 通过（本轮改动后已复跑，`CARGO_EXIT=0`）。
- PR-08/09 为稳态零逐帧分配：缓冲池空或首次回调仍会分配；尚未用 dhat 建立分配基线。

---

## 2. 当前设计决策（已确认）

| 项 | 决定 |
|---|---|
| 依赖层 | 不修改 `tsclientlib/tsproto`、不 fork、不打补丁 |
| jitter buffer | **固定 60 ms（3 帧）**，迟到即丢，不随网络增长 |
| 解码器池 | 空闲 **5 分钟**淘汰；上限 **512**，LRU；`UserLeft` 立即清理 |
| 音频事件负载 | `Arc<[u8]>`，广播与返回事件共享，不再深拷贝 |
| 上传完成判定 | 以 `ftgetfileinfo` 落盘大小为准；2568 无权限仅 warning |
| 敏感信息 | `ClientConfig`/Builder 的 `Debug` 输出 `<redacted>` |
| `max_memory_download` | 默认 **64 MiB**；超出报 `FileTransferFailed`，不截断 |
| 初始静音 | 默认 **true**（隐私安全），暂未做成 `ClientConfig` 字段 |
| 组 whisper | 本轮不做 |
| 命令结果等待 | 暂缓，需 actor/独立事件泵 |
| Go/No-Go | 高风险低收益不做 |

---

## 3. 剩余 backlog

### P1（低风险、建议下一批）

1. **FR-BD-05**：JNI `FileList` 改 JSON 库序列化；复查绑定层日志脱敏。
2. **PR-04**：`users()`/`channels()` 快照化（`Arc<[...]>` 或迭代器），先定 API 方案。
3. **PR-06/07**：事件总线满载时控制事件不丢；慢 handler 不阻塞事件泵（需 actor 设计）。
4. **PR-10/11**：音频回调锁最小化、解码器分片锁/复用缓冲（需真机基准）。

### P1（需要设计/较大改动）

- **FR-CN-02 命令结果**：先做 actor + 事件泵，再实现 `try_*` async 方法；不要在当前 `&mut Client` 上直接 await。
- **完整事件流**：每消费者 `EventStream`、`notification` 兜底、慢消费者策略（同 actor）。
- **FR-FS-03 流式/续传**：`download_file_to_path`/`upload_file_from_path`、`resume`/`seek`。
- **音频管线**：FR-AU-03/04/05/07/08（AEC 接入、重采样、jitter、PLC、设备格式）。

### P2（功能对齐，暂缓）

- ServerQuery、TS6 Streams、管理命令层、头像/图标、完整 view/revision、客户端版本选择、组 whisper、Identity 格式互通/`uid(algorithm)`、trace/diagnostics、fuzz/协议文档。

### 验证遗留

- Android x86_64 交叉检查。
- 真实 TS3/TS6 服务器集成测试。
- Android 真机音频/AEC/功耗测试。
- `docs/perf-baseline.md`、`docs/protocol-behavior.md`。
- CI `cargo-semver-checks` / `cargo public-api`。
- Java JUnit/仪器测试。

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
