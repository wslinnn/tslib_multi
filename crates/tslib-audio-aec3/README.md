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
引擎配置只开 AEC3——NS/AGC/HPF 全关：降噪由应用管线（RNNoise）负责，双重
处理会让两边都变差；采集维持裸 PCM（VOICE_PERFORMANCE），全链路软件处理。
