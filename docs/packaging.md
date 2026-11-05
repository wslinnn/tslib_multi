# 发布与打包

> 对应施工项：C6-12。CI 工作流见 `.github/workflows/android.yml` 与 `release.yml`。

## Android（JNI / AAR 输入）

```bash
export ANDROID_NDK_HOME=/path/to/ndk
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=...
cargo ndk --platform 29 -t arm64-v8a -t x86_64 \
  -o dist/jniLibs build -p tslib-jni --release --features vendored-openssl
```

- 输出 `dist/jniLibs/<abi>/libtslib_jni.so`。
- Java 源码在 `java/src/main/java/dev/tslib`，随 App/AAR 一起编译。
- 发布构建日志级别固定为 `Info`（debug 为 `Trace`）。

## C / FFI

```bash
cargo build -p tslib-ffi --release
# header 由 build.rs/cbindgen 生成到 crates/tslib-ffi/include/tslib.h
```

## Python wheel

```bash
pip install maturin
maturin build -m crates/tslib-python/Cargo.toml --release
```

## 版本与变更

1. 更新 `Cargo.toml` workspace version 与 `CHANGELOG.md`。
2. 公开 API 变更写入 `docs/api-changes/v<version>.md`。
3. `cargo-semver-checks`（`.github/workflows/api.yml`）必须通过。
4. 打 tag 后由 `release.yml` 产出构建物。

## 检查清单

- [ ] 所有 crate `cargo test -j 1` 通过。
- [ ] `cargo check -p tslib-ffi -p tslib-python -p tslib-jni` 通过。
- [ ] Android arm64/x86_64 交叉检查通过。
- [ ] CHANGELOG 与迁移文档已更新。
