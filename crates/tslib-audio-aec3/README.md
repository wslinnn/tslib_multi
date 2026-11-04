# tslib-audio-aec3

WebRTC AEC3 回声消除引擎（`tslib_audio::echo::EchoEngine` 实现）。

## 来源与许可

`c/wap/` vendored 自 freedesktop **webrtc-audio-processing 2.1** 官方发布 tarball
（https://www.freedesktop.org/software/pulseaudio/webrtc-audio-processing/webrtc-audio-processing-2.1.tar.xz），
PulseAudio/PipeWire 维护者打包的上游是 WebRTC（Google）`AudioProcessing` 模块的
独立发行版。许可 **BSD-3-Clause**（见 `c/wap/COPYING`），未改动任何源文件。

- abseil 依赖经 meson subproject 构建（`subprojects/abseil-cpp.wrap`），
  对应源码与 wrapdb 补丁已预取进 `subprojects/packagecache/`（sha256 与
  .wrap 声明一致，CI 离线可复现）。
- meson 会把 abseil 解压到 `subprojects/abseil-cpp-20240722.0/` 并在
  `c/wap/build/` 生成构建树——两处均为生成物，build/ 已 gitignore。

## 构建

需要 PATH 里有 **meson（>=0.63）+ ninja**：

- 宿主 Windows：MSVC 环境（`vcvars64`）下跑 cargo；`scoop install meson` 即可
- 宿主 Linux（CI 测试）：`apt install meson ninja-build`
- Android（arm64-v8a / x86_64）：`ANDROID_NDK_HOME` 指向 NDK，build.rs 生成
  meson 交叉文件（统一 clang 驱动 + `--target`，API 29 = app minSdk，
  libc++ 以静态归档 `-l:libc++.a` 精确名链接——`-lc++` 会选中共享版并引入
  APK 未打包的 libc++_shared.so 运行时依赖，导致启动 dlopen 失败）；CI（ubuntu）
  负责出双 ABI `.so`

一个已知的上游兼容点：上游 `meson.build` 声明 `cpp_std=c++17`，但其代码里
`input_volume_stats_reporter.cc` 使用了指定初始化器，MSVC 严格要求 C++20
（error C7555）；GCC/Clang 只是把 C++20 语法当 C++17 扩展放行。本 crate 统一
在 meson 命令行传 `-Dcpp_std=c++20` 覆盖（首次 configure 的命令行选项优先于
project 默认值），不改上游任何源文件。

## C 包装层

`c/wrapper/aec3_wrapper.cpp` 是几十行的 C ABI shim，直接调
`webrtc::AudioProcessing`（ProcessReverseStream + ProcessStream，10ms 块）。
引擎配置只开 AEC3 + 固定 +6dB 采集后置增益（`capture_level_adjustment`
的 `post_gain_factor`，APM 在全部采集处理之后缩放输出）——NS/AGC/HPF 全关：
降噪由应用管线（RNNoise）负责，双重处理会让两边都变差；采集维持裸 PCM
（VOICE_PERFORMANCE），全链路软件处理。

两个增益相关的选型依据（均有 spike 实测数据，见测试）：

- **后置而非前置**：裸 PCM 普遍偏安静，+6dB 补偿发送响度。增益放 AEC 前
  会把噪声地板一并抬进 AEC3 抑制器的判定区，实测 static ERLE 19.9→13.0dB；
  放在链路最末端（`ApplyPostLevelAdjustment`）与消回声零交互，归一化后
  指标与基线完全一致（19.9/14.7/30.5）。
- **不用 AGC2**：AGC2 在 AEC 之后运行，RNN VAD 会把"语音形状"的残余回声
  当成语音放大，远端放音+本端静音时残余回声被抬高约 20dB（实测 static
  ERLE 19.9→1.1dB）——自回声回归不可接受。

`tslib_aec3_post_gain()` 暴露该常量；防炸看门狗
（`tslib_audio::echo::sanitize_echo_output`）与 spike 测试均按它归一，
保证增益不污染抑制能力指标。
