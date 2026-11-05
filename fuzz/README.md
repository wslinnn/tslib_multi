# Fuzz targets

需要 nightly 与 `cargo-fuzz`：

```bash
cargo install cargo-fuzz
cargo +nightly fuzz run bbcode
```

当前覆盖 BBCode 解析/渲染（HTML/纯文本/ANSI）的 panic/无限循环问题。
