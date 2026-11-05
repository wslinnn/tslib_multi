# API 变更记录（下游平滑升级）

本目录用于记录每次发布对**公开 API**的变更，重点是**方法入参、返回结构、字段类型、行为语义**，方便 Android/Python/C 下游项目按文档迁移。

## 1. 什么时候必须写

只要满足任一条，就必须在本目录新增/更新对应版本的差异文档：

- 新增、删除、重命名公开方法/函数/类型/字段/枚举变体。
- 修改方法参数、参数顺序、返回类型、错误类型。
- 修改结构体字段类型、字段含义、可选性。
- 签名不变但行为变化：默认值、超时、事件时序、错误语义、线程要求。
- 绑定层（JNI/FFI/Python）符号、签名、线程模型变化。

纯内部重构、性能优化（外部行为不变）不需要单列 API 变更，但仍要在 CHANGELOG 说明。

## 2. 文件命名

| 文件 | 用途 |
|---|---|
| `TEMPLATE.md` | 模板，不直接修改 |
| `v<version>.md` | 每个发布版本一份，例如 `v0.2.0.md` |
| `README.md` | 本说明 |

一个版本内多次改动合并到同一份文件；发布后不再修改历史文件（勘误除外）。

## 3. 变更分级

| 级别 | 定义 | 下游动作 |
|---|---|---|
| A（Additive） | 新增公开项；枚举已 `#[non_exhaustive]` | 无需改 |
| B（Behavioral） | 签名不变，语义/默认值/时序变化 | 建议检查 |
| C（Breaking signature） | 参数/返回/字段类型/结构布局变化 | 必须改 |
| D（Removal） | 删除公开项 | 必须迁移 |

`0.x` 版本允许 C/D，但必须提前一个 minor 标记 deprecated，并给出迁移片段。

## 4. 流程

1. **开发前**：在 issue/spec 标注变更级别（A/B/C/D）。
2. **实现时**：同步填写 `docs/api-changes/v<version>.md`，不要等发布前补。
3. **生成机器 diff**：
   ```bash
   # Rust 公开 API（需安装 cargo-public-api）
   cargo public-api --diff > target/api-diff/rust.txt

   # 语义检查（需安装 cargo-semver-checks）
   cargo semver-checks check-release

   # C header
   cargo build -p tslib-ffi
   # 对比 crates/tslib-ffi/include/tslib.h 与上一版

   # Java
   javap -s -p -classpath <旧>:/path <类> > old.txt
   javap -s -p -classpath <新>:/path <类> > new.txt
   diff old.txt new.txt

   # Python
   python -c "import tslib; help(tslib)" > new.txt
   diff old.txt new.txt
   ```
4. **Review**：逐条核对“旧/新/迁移/绑定影响/行为变化”。
5. **发布**：CHANGELOG 对应版本链接到本文件；release notes 写升级步骤。

## 5. 每条记录必须回答

- 旧签名/旧结构是什么？
- 新签名/新结构是什么？
- 变更级别是什么？
- Rust/Java/Python/C 分别怎么改？给可粘贴片段。
- 签名没变但行为变了吗？
- 旧符号保留到哪个版本？
- 对应测试/示例在哪？

字段级变更用表格，不要写“字段有调整”。

## 6. 弃用策略

```rust
#[deprecated(
    since = "0.2.0",
    note = "use try_move_to_channel; removed in 0.4.0"
)]
pub fn move_to_channel(...) -> Result<()> { ... }
```

- 至少保留一个 minor 周期。
- 旧绑定符号（JNI native 名、C 函数、Python 方法）同样保留一个周期。
- 移除前在差异文档中标注“本轮移除”。

## 7. Review 清单

- [ ] 所有 C/D 级变更都有迁移片段。
- [ ] 所有 B 级变更都在“行为语义变更”一节。
- [ ] 返回结构有字段级对照表。
- [ ] Java/Python/C 影响逐项列出。
- [ ] 旧/新示例至少各编译一次。
- [ ] 机器 diff 与人工文档一致。
- [ ] CHANGELOG 已链接。
- [ ] 没有只改签名、不写迁移的条目。

## 8. 与其它文档的关系

| 文档 | 回答 |
|---|---|
| `docs/spec-error-fixes-performance.md` | 为什么做、做什么、验收 |
| 本目录 `v<version>.md` | 旧/新 API、怎么迁移、绑定影响 |
| `CHANGELOG.md` | 一句话摘要 + 链接 |
| Release notes | 用户视角升级步骤 |
