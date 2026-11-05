# 项目工作规范（tslib_multi）

> 本文件是本仓库的持久上下文，compact/新会话后先读这里和 `docs/handoff.md`。
> 全局规范仍以 `C:\Users\LINWSH\.zcode\AGENTS.md` 为准。

## 1. 项目定位

Rust workspace，TeamSpeak 3/6 客户端库，基于 `tsclientlib/tsproto`（固定 rev `3fbfa26`）。

| crate | 职责 |
|---|---|
| `tslib-core` | 连接、身份、事件、状态、文件、权限 |
| `tslib-audio` | cpal 采集/播放、Opus、VAD、混音、speex 回声消除 |
| `tslib-audio-aec3` | vendored WebRTC AEC3 |
| `tslib-chat` | BBCode/Markdown、消息历史 |
| `tslib-channel` | 频道树/管理器 |
| `tslib-bot` | Bot/命令/插件 |
| `tslib-ffi` / `tslib-python` / `tslib-jni` | C/Python/Java 绑定 |

## 2. 硬约束

- **不修改 `tsclientlib/tsproto`、不 fork、不打补丁**。只使用公开 API（`send_with_result`、`ConnectOptions` 等）。
- 需要改依赖层才能做的项：记录为限制或绕行，不做 fork。
- **高风险低收益不做**：actor/命令结果等待、完整每消费者事件流、ServerQuery/Streams、破坏性 `maxclients`/`target_id` 等，先搁置。
- `Client` 是 `!Send/!Sync`；不要在 `&mut Client` 上 await 命令结果（会死锁）。命令结果需要 actor/独立事件泵。
- 公开 API 变更必须更新 `docs/api-changes/v<version>.md` 与 `CHANGELOG.md`。

## 3. 本机环境（D:\develop\env）

| 工具 | 路径 |
|---|---|
| JDK 21 | `D:\develop\env\jdk\jdk21.0.4_7` |
| JDK 17 | `D:\develop\env\jdk\jdk17.0.12_7` |
| Android NDK | `D:\develop\env\android-sdk\ndk\27.2.12479018` |
| ninja | `D:\develop\env\android-sdk\cmake\3.22.1\bin` |
| meson | `C:\Users\LINWSH\scoop\shims` |
| Perl 模块（OpenSSL） | `D:\develop\env\.perl-extras` |
| MSVC | VS BuildTools 18，`...\VC\Auxiliary\Build\vcvars64.bat` |

### 3.1 常规测试

```bash
# 本机默认并行 cargo 偶发 rustc STATUS_STACK_BUFFER_OVERRUN，统一用 -j 1
cargo test -j 1 -p tslib-core -p tslib-bot -p tslib-audio -p tslib-channel -p tslib-chat
cargo check -p tslib-ffi -p tslib-python -p test-suite
```

### 3.2 Host JNI

需要 MSVC 环境 + ninja/meson：

```bat
call "C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
set "PATH=D:\develop\env\android-sdk\cmake\3.22.1\bin;D:\develop\env\jdk\jdk21.0.4_7\bin;C:\Users\LINWSH\scoop\shims;%PATH%"
set "ANDROID_NDK_HOME=D:\develop\env\android-sdk\ndk\27.2.12479018"
cargo check -p tslib-jni
cargo build -p tslib-jni
dumpbin /exports target\debug\tslib_jni.dll | findstr Java_dev_tslib_
```

### 3.3 Java 编译

```bash
export PATH="/d/develop/env/jdk/jdk21.0.4_7/bin:$PATH"
javac -d target/java-classes java/src/main/java/dev/tslib/*.java
javap -s -p -classpath target/java-classes dev.tslib.Client
```

### 3.4 Android arm64 JNI

必须 `--features vendored-openssl`。OpenSSL 的 Perl 模块需要 `PERL5LIB`；CMake 需要 Ninja 与 arm64 ABI 包装 toolchain。

```bat
set "PATH=D:\develop\env\android-sdk\cmake\3.22.1\bin;D:\develop\env\jdk\jdk21.0.4_7\bin;C:\Users\LINWSH\scoop\shims;%PATH%"
set "ANDROID_NDK_HOME=D:\develop\env\android-sdk\ndk\27.2.12479018"
set "ANDROID_NDK_ROOT=D:\develop\env\android-sdk\ndk\27.2.12479018"
set "PERL5LIB=D:\develop\env\.perl-extras"
set "CMAKE_GENERATOR=Ninja"
set "CMAKE_MAKE_PROGRAM=D:\develop\env\android-sdk\cmake\3.22.1\bin\ninja.exe"
set "CMAKE_TOOLCHAIN_FILE=D:/develop/project/TS6_Droid_CN/tslib_multi/target/android-toolchain.cmake"
cargo ndk -t arm64-v8a check -p tslib-jni --features vendored-openssl
echo CARGO_EXIT=%ERRORLEVEL%
```

`target/android-toolchain.cmake` 是本地辅助文件（target 忽略），内容：

```cmake
set(ANDROID_ABI arm64-v8a)
set(CMAKE_ANDROID_ARCH_ABI arm64-v8a)
include("D:/develop/env/android-sdk/ndk/27.2.12479018/build/cmake/android.toolchain.cmake")
```

arm64 已通过；x86_64 尚未跑。

## 4. 踩坑

- **不要 `cargo fmt --all`**：本仓库 rustfmt/行尾差异会重排全仓库，产生大量无关 diff。只格式化改动的文件，或手动保持格式。
- 对单个文件跑 `rustfmt` 会顺着 `mod` 声明递归格式化子模块；跑完用 `git status` 检查，`git checkout --` 掉未改动的文件。
- 默认并行 `cargo test` 偶发 rustc `STATUS_STACK_BUFFER_OVERRUN`；用 `-j 1`。
- `Cargo.lock` 被 `.gitignore` 忽略，不要强行提交。
- Android `cargo ndk` 失败时看清楚 `CARGO_EXIT`；不要只看 shell 退出码。
- `tsclientlib` 的语音加密固定为开；TS6 文件时间、接收窗口 ACK/空洞、文件 `proto=1`、`capability_extensions` 是依赖层限制，不改 D 只能接受/绕行。
- `send_with_result` 能拿到 `MessageHandle`，但等待结果需要独立事件泵；不要直接在当前 `Client` 上 await。
- 文件下载默认内存上限 64 MiB、idle timeout 30 s、chunk 64 KiB；大文件不要静默截断。

## 5. 当前状态与计划

- 详细需求/验收：`docs/spec-error-fixes-performance.md`（v1.1）
- 对比与借鉴：`docs/teamspeak-js-learnings.md`
- 迁移/API 差异流程：`docs/api-changes/README.md`，当前版本 `docs/api-changes/v0.2.0.md`
- 变更摘要：`CHANGELOG.md`
- 当前进度/剩余 backlog：`docs/handoff.md`

**已完成**：B1 全部；B2 大部分（Connected 单次、配置接线、非 Opus codec、Bot 事件泵、并发查询、移动状态、重连恢复、FFI 事件泵、Java 缺失 native/EchoCanceller、User/ServerInfo 字段、kick/ban/ChannelJoined、权限提示、is_talking、热路径日志）；B4 低风险项（解码器清理、说话超时 O(log n)、采集/播放/混音缓冲复用、音频事件 `Arc` 负载）；FR-SC-02 配置 Debug 脱敏；FR-FS-06 上传落盘校验；Python 历史写入/实时 server info/事件负载。

**未做/暂缓**：

1. 命令结果等待（actor，暂缓）
2. 文件流式/续传、`ftstop/ftlist`（上传校验已完成）
3. 音频 jitter/PLC/重采样/AEC 集成/设备格式（解码器清理已完成）
4. 性能 PR-01/04/06/07/10~16（PR-02/03/08/09 已完成；PR-08/09 为稳态零逐帧分配）
5. Python 长期专用线程/GIL 方案、JNI FileList JSON/日志脱敏
6. `PermissionHints.known`、`maxclients` 语义、`joinable_channels` 人数
7. ServerQuery/Streams/命令层/头像图标/完整 view 等 P2
8. Android x86_64、真实服务器/真机、性能基线、CI semver 门禁、Java 测试

**下一批建议**：FR-BD-05 JNI FileList JSON、PR-04 快照 API 设计、PR-06/07 事件总线、PR-10/11 音频锁与解码器分片。
