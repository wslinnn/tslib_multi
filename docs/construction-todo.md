# 施工 Todo（tslib_multi）

> 依据：`docs/spec-error-fixes-performance.md`（v1.1）、`docs/teamspeak-js-learnings.md`、`docs/handoff.md`。  
> 范围规则：**需要修改 `tsclientlib/tsproto` 的项不做**（见文末“不做”）；其余全部列入本清单。  
> API 规则：方法入参/返回结构**可以改**，但凡公开 API 变更必须同步更新 `docs/api-changes/v0.2.0.md` 与 `CHANGELOG.md`。  
> 最后更新：2026-11-05

## 0. 已拍板决策

| 决策 | 结论 |
|---|---|
| PR-04 返回类型 | `users()` / `channels()` 改为返回 `Arc<[User]>` / `Arc<[Channel]>`（允许破坏，需写迁移文档） |
| 命令与事件架构 | 分阶段 actor：先骨架与事件泵，再迁移关键命令到可等待结果 |
| FR-FS-03 文件流式 | 路径式：`download_file_to_path` / `upload_file_from_path`，支持 offset/resume |
| 下一批 | **C1 正确性收尾** |

图例：`[ ]` 未开始，`[~]` 部分完成，`[x]` 已完成（仅保留需要跟踪的收尾项）。

---

## C1. 正确性收尾（下一批）

- [x] **C1-1 FR-CN-03 配置剩余项**
  - `ClientConfig.version: Option<String>`（或依赖层 `Version`），接入 `ConnectOptions::version`。
  - `ClientConfig.initial_input_muted: bool`，默认 `true`，替换当前硬编码。
  - `auto_reconnect` / `reconnect_delay` / `max_reconnect_attempts` 加 `#[deprecated]`，文档说明“由依赖层处理，B 不保证”。
  - 同步 Java/Python 配置入口（如已暴露）；写 API 迁移。
  - 验收：可切换客户端版本；初始静音可配置；弃用项有 note。

- [x] **C1-2 FR-EV-02 文本消息目标 id**
  - `Event::TextMessage` 增加 `target_id: Option<u64>`：Server=`None`，Channel=频道 id，Private=目标 client id。
  - 频道消息从 invoker 所在频道或消息目标补全。
  - JNI/Python 事件字典同步；写 API 迁移。
  - 验收：频道消息可拿到频道 id，私聊可拿到目标 id。

- [x] **C1-3 FR-EV-03 剩余用户字段**
  - `User` 增加 `created_at` / `last_connected_at`，映射依赖层 `optional_data.created/last_connected`。
  - Java/Python 绑定同步；写 API 迁移。
  - 验收：服务器有值时字段非空。

- [x] **C1-4 PR-04 破坏性快照**
  - `users()` → `Arc<[User]>`，`channels()` → `Arc<[Channel]>`；缓存失效点保持（事件泵/`sync_state`/`set_input_muted`）。
  - `users_snapshot()` / `channels_snapshot()` 作为过渡别名（可 `#[deprecated]`）。
  - 更新 JNI、Python、FFI、examples、test-suite。
  - 写 `docs/api-changes/v0.2.0.md`：旧/新签名、返回结构、迁移片段、绑定影响。
  - 验收：所有 crate 编译；快照共享测试通过；绑定不再全量克隆。

- [x] **C1-5 绑定层快照适配**
  - JNI `nativeGetUsers/nativeGetChannels` 使用新返回类型，避免重复 `users()` 调用。
  - Python `Client.users()/channels()` 适配 `&mut self` 与 `Arc`，`ChannelTree.from_channels` 同步。
  - 验收：Android/Java/Python 编译与现有测试通过。

- [x] **C1-6 FR-ST-05 旧接口收尾**
  - 决定并实现 `joinable_channels` 的人数语义：改为接收用户列表/占用闭包（破坏）或标 `#[deprecated]` 指向 `joinable_channels_for_users`。
  - 写迁移文档。
  - 验收：满员频道在推荐入口中被排除。

- [x] **C1-7 L2 接收 watchdog（B 侧缓解）**
  - 记录每个说话用户最后音频时间；超过阈值发 `TalkStatusStop`/连接质量事件（不修改 D）。
  - 阈值可配置，默认不误伤正常 20 ms 帧。
  - 验收：模拟长时间无包时状态能恢复；不产生重复事件。

---

## C2. 命令与事件架构（分阶段 actor）

- [x] **C2-1 actor 骨架**
  - 专用线程/任务持有 `Client`（`!Send`），命令通道 + 事件泵 + 关停协议。
  - 定义命令/结果/超时/错误类型。
  - 验收：actor 内可连续 pump；关停不悬挂。

- [x] **C2-2 FR-CN-02 关键命令返回结果**
  - 发消息、移动频道、静音、踢人/封禁、频道编辑等返回成功/失败/超时。
  - 提供 `try_*`/async 包装；旧 fire-and-forget 方法保留或标弃用。
  - 验收：服务器拒绝时调用方可感知；超时可配置。

- [x] **C2-3 PR-06 事件总线不丢控制事件**
  - 控制事件用可靠队列；语音事件允许 drop-oldest 并计数。
  - 验收：满载下控制事件不丢；丢弃计数可查询。

- [x] **C2-4 PR-07 慢 handler 不阻塞事件泵**
  - handler 移出泵线程（每消费者队列/worker），慢消费者有丢弃或背压策略。
  - 验收：注入慢 handler 时事件泵 p99 不受影响。

- [x] **C2-5 完整事件流**
  - `Event::Notification` 兜底；每消费者 `EventStream`；`Lagged` 策略。
  - 验收：多消费者互不影响；未知事件不丢。

- [x] **C2-6 Python 专用线程**
  - 连接与事件循环移到后台线程，Python 侧非阻塞 API；GIL 释放说明更新。
  - 验收：Python 主线程阻塞调用不再卡住事件处理。

- [x] **C2-7 FFI/JNI actor 适配**
  - 现有 `pump/waitEvents` 兼容层；命令结果回调或 future 映射。
  - 验收：C/Java 调用方可拿命令结果。

---

## C3. 文件传输

- [x] **C3-1 FR-FS-03 路径式流式/续传**
  - `download_file_to_path(channel, path, dest, offset)` / `upload_file_from_path(...)`。
  - 支持 resume/seek；大文件不整块进内存；idle timeout 沿用。
  - 验收：>64 MiB 文件可下载/上传；断点续传可用。

- [x] **C3-2 文件命令补齐**
  - 公开 `ftstop`、`ftlist`、`ftgetfileinfo`（当前仅内部上传校验）。
  - 验收：可停止传输、查询传输列表与单文件信息。

- [x] **C3-3 文件集成测试**
  - 覆盖密码频道、并发列表、上传校验、续传、失败回滚。

---

## C4. 音频硬化

- [ ] **C4-1 FR-AU-05 重采样**
  - 采集：设备采样率 → 48 kHz；播放：48 kHz → 设备采样率；使用 `rubato`。
  - 设备为 48 kHz 时旁路。
  - 验收：44.1 kHz 设备可用，音高/时长不变。

- [ ] **C4-2 FR-AU-08 设备格式协商**
  - 查询支持配置，优先 48 kHz/单声道；不支持则选最接近并启用重采样。
  - 失败错误包含设备名与支持格式。
  - 验收：常见 44.1 kHz 设备可打开；错误可读。

- [ ] **C4-3 FR-AU-07 AEC 接入 AudioManager**
  - `AudioConfig.echo_cancellation` 改枚举 `Disabled/Speex/Aec3`。
  - 采集近端 + 播放远端参考接入；默认 `Disabled`。
  - 验收：AEC 开启后回声明显下降；关闭时无额外延迟。

- [ ] **C4-4 AGC/NS 接线**
  - `agc_enabled` / `agc_target` / `noise_suppression` 当前无消费点，接入采集链路。
  - 验收：配置变化可观测；默认值不破坏现有音量。

- [ ] **C4-5 PR-11 解码器分片锁/复用缓冲**
  - 按用户分片锁或复用 `samples` 缓冲，减少 `process_incoming` 分配与锁竞争。
  - 验收：多线程调用时争用下降；单线程无回退。

- [ ] **C4-6 PR-12 播放缓冲与端到端延迟**
  - `playback_buffer_ms` 默认从 60 调到 ≤40；建立回环延迟测试。
  - 验收：p95 ≤120 ms（含 60 ms jitter）。

- [ ] **C4-7 时钟漂移**
  - 长期通话中设备时钟与 48 kHz 基准漂移处理（丢/补帧策略）。
  - 验收：30 分钟通话 underrun/overflow 可控。

- [ ] **C4-8 Android 音频路由钩子**
  - audio focus、蓝牙/有线切换、网络切换的接口与文档（SDK 提供钩子，App 侧实现）。
  - 验收：切换设备不崩溃、音频可恢复。

---

## C5. 性能、可观测与 CI

- [ ] **C5-1 PR-01 事件循环测量**
  - 内置计时：控制事件投递 p99、`process_events` 单次耗时；必要时再优化。
  - 验收：p99 ≤10 ms、单次 ≤16 ms（或记录基线差异）。

- [ ] **C5-2 PR-02 事件分配测量**
  - dhat 统计每事件堆分配（不含 payload），目标 ≤1；音频 payload 复制 0。
  - 验收：报告写入 `docs/perf-baseline.md`。

- [ ] **C5-3 PR-08/09 音频分配测量**
  - dhat 验证采集/播放/mixer 稳态 0 分配；记录首次/池空例外。
  - 验收：报告入库。

- [ ] **C5-4 PR-13 事件与队列可观测**
  - 事件丢弃、语音 drop-oldest、队列深度计数与查询接口。
  - 验收：满载/慢消费者场景可观测。

- [ ] **C5-5 PR-16 基准**
  - criterion（核心热路径）+ Android Perfetto 30 分钟通话。
  - 验收：`docs/perf-baseline.md` 有可复现数据。

- [ ] **C5-6 FR-DOC-03 CI 门禁**
  - CI 增加 `cargo-semver-checks` / `cargo public-api`，差异作为迁移文档输入。
  - 验收：PR 上能看到 API 差异。

---

## C6. 产品能力（P2，按产品节奏）

- [ ] **C6-1 类型化 ID / `FieldUpdate` / `raw` 字段**
- [ ] **C6-2 协议常量表与权限 id 去硬编码**
- [ ] **C6-3 完整 `view` + revision + `view_ready`/`view_changed`**
- [ ] **C6-4 管理命令层**（频道/客户端/群组/权限/封禁，按 MoSCoW 裁剪）
- [ ] **C6-5 头像/图标全生命周期**（上传/下载/删除、文件名推导、大小权限）
- [ ] **C6-6 Identity 双格式互通 + 160 字符上限**
- [ ] **C6-7 `exec_raw`**（原始命令发送）
- [ ] **C6-8 组 whisper**：先做可行性 spike（公开 `C2SWhisperNew`/`send_audio` 能否构造）；若必须改 D 则移入“不做”
- [ ] **C6-9 ServerQuery crate**（TCP/SSH/WebQuery，独立 crate）
- [ ] **C6-10 TS6 Streams crate**（屏幕共享信令；独立 crate，先做协议可行性评估）
- [ ] **C6-11 trace/diagnostics/指标导出**
- [ ] **C6-12 打包**：AAR、wheel、C header、版本号与发布流程
- [ ] **C6-13 fuzz 与协议实测文档**

---

## C7. 验证与质量

- [ ] **C7-1 真实 TS3/TS6 服务器集成测试**（上传校验、重连、权限、文件、语音）
- [ ] **C7-2 Android 真机音频/AEC/功耗/30 分钟通话**
- [ ] **C7-3 `docs/perf-baseline.md`**（硬件、构建、测量方法、结果）
- [ ] **C7-4 `docs/protocol-behavior.md`**（实测行为、差异、依赖层限制）
- [ ] **C7-5 Java JUnit/仪器测试**
- [ ] **C7-6 绑定回归矩阵**（host JNI、Java、Python、Android arm64/x86_64）

> arm64/x86_64 交叉检查已完成，后续改动继续复跑。

---

## 依赖顺序

1. C1 → C2（actor 需要 C1 的 API 稳定）。
2. C2 是 C3 命令结果、C2-6 Python 线程、C5-4 事件计数的前置。
3. C3 / C4 可并行（文件与音频互不依赖）。
4. C5-6（CI 门禁）可提前做；C5-1/2/3 依赖对应实现完成。
5. C6 按产品优先级插入，不与 C1–C5 争抢同一模块。
6. C7 贯穿各批次，真实服务器/真机在 C4 后集中做。

## 不做（需要修改 tsclientlib/tsproto）

| 项 | 原因 |
|---|---|
| 语音加密跟随 `codec_encryption_mode` | 依赖层固定 `voice_encryption=true` |
| 接收窗口 ACK/空洞超时的根治 | 依赖层算法；本轮只做 C1-7 watchdog 缓解 |
| TS6 文件时间保留原始值 | 依赖层按秒解析 |
| 握手版本/puzzle 上限 | 依赖层固定 |
| 文件 `proto=1` | 依赖层硬编码 |
| `capability_extensions` | 依赖层未进 book |
| `clientek` 判定 | 依赖层首包逻辑 |
| `Client` 改为 `Send/Sync` | 依赖层架构；B 侧用 actor 规避 |

> 组 whisper / TS6 Streams 若可行性评估确认必须改依赖层，则移入本表。
