# 性能基线

> 最后更新：2026-11-05  
> 对应需求：PR-01/02/04/08/09/13/16（`docs/spec-error-fixes-performance.md`）  
> 说明：本文件记录可复现的软件侧基线；Android 真机/真实服务器数据在 C7 补齐。

## 1. 环境

| 项 | 值 |
|---|---|
| OS | Windows 10.0.26300 x64 |
| Rust | stable（workspace 当前工具链） |
| 构建 | `--release`（bench profile） |
| CPU | 本机开发机（未固定频率） |

## 2. 运行方式

```bash
# 频道树：200 频道构建、路径/上限查询
cargo bench -p tslib-channel --bench tree

# jitter buffer：10k 语音包 push + pump
cargo bench -p tslib-audio --bench jitter

# 分配计数：事件 clone / mixer 回调
cargo bench -p tslib-core --bench allocations
cargo bench -p tslib-audio --bench allocations
```

事件泵统计通过 `Client::pump_stats()` 读取（`pumps`、`last_events`、`last_duration_us`、`max_duration_us`），在真实连接下采样 p99。

## 3. 当前结果（2026-11-05）

| 场景 | 结果 | 目标 | 备注 |
|---|---|---|---|
| `ChannelTree::from_channels`（200 频道） | 2.44 ms/次 | PR-04：200 频道 + 1000 用户 `sync_state` ≤20 ms | 只含频道；用户映射未计入 |
| `path_to` + `effective_max_clients` | 119 ns/次 | 查询无全量克隆 | — |
| jitter `push` + `pump` | 110 ns/包 | 实时 50 包/秒/人余量充足 | 10k 包平均 |
| 默认延迟预算 | 60 ms jitter + 40 ms 播放 = 100 ms | PR-12：p95 ≤120 ms | 软件预算；端到端待真机回环 |
| mixer 空缓冲 | `PlaybackStats.underruns` 可查询 | PR-13 | — |
| actor 事件队列 | `pending_control_events` / `pending_voice_events` / `dropped_voice_events` | PR-13 | 控制事件无界不丢；语音 drop-oldest |
| `AudioReceived` clone（含 `Arc` payload） | 0 分配/clone | PR-02 | 计数分配器，100k 次 |
| `AudioMixer::mix` 稳态 | 0 分配/回调 | PR-09 | 计数分配器，10k 次（预热后） |

## 4. 分配验证（PR-02/08/09）

已用自定义计数分配器验证：

- `AudioReceived` 事件 clone：0 分配/clone（payload 为 `Arc<[u8]>`）。
- `AudioMixer::mix` 稳态：0 分配/回调（临时缓冲预热后复用）。

仍需在真机/设备回调中复核：

- 采集回调稳态 0 分配/帧（缓冲池空或首次回调除外）。
- 播放回调稳态 0 分配（重采样/声道映射路径除外，该路径会分配临时 Vec）。
- 每事件堆分配（不含 payload）≤1（文本/状态事件仍会克隆字符串）。

## 5. 待补

- Android Perfetto 30 分钟通话：CPU <5%、内存增长 <5 MB、underrun <1/min。
- 真实 TS3/TS6 服务器下 `process_events` p99 / `wait_events` 控制事件投递延迟。
- 1000 用户/200 频道 `sync_state` 完整计时。
