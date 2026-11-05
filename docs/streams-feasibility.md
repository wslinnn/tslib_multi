# TS6 Streams 可行性评估

> 对应施工项：C6-10  
> 结论：**暂不实现**，除非产品明确要求 TS6 屏幕共享。

## 现状

- `tsclientlib/tsproto` 当前固定版本没有公开 Streams 信令 API。
- Streams 涉及独立的建流/协商协议（WebRTC/SSRC、编解码、权限），不在现有 `InMessage`/`StreamItem` 覆盖范围内。
- 该能力无法只靠公开 API 完成；绕过依赖层意味着自行实现一套协议栈。

## 可选方案

| 方案 | 成本 | 风险 |
|---|---|---|
| 只做 UI 占位/能力检测 | 低 | 无实际功能 |
| 独立 crate 逆向/实现 Streams 信令 | XL | 协议未文档化，兼容性不可保证 |
| 修改 tsclientlib/tsproto | — | 违反当前“不 fork”约束 |

## 决策

1. 不在 C6 内实现 Streams。
2. 保留本文件作为可行性记录；若产品要求，先做独立 crate 的协议 spike，再评估是否允许依赖层改动。
3. 现有语音/文件/ServerQuery 能力不受影响。
