# Zero-Copy Audio Buffering Optimization

This PR resolves a critical memory allocation bottleneck in the real-time audio bridge by refactoring `SmartAudioBuffer` to achieve true zero-allocation on the hot path while maintaining exact backwards compatibility.

## Changes Overview

1.  **Refactored `SmartAudioBuffer` state**:
    *   Replaced the internal `Vec<i16>` with `bytes::BytesMut`.
    *   Using `BytesMut` permits O(1) buffer slicing (`split_to().freeze()`), preventing data reallocation when generating standard audio chunks from the buffer pool.
2.  **Zero-Allocation Chunk Extractions**:
    *   Added `pop_chunk(format)` and `pop_remaining_chunk(format)`. These natively return `AudioChunk` structures by passing raw memory directly from the `BytesMut` split logic.
3.  **Correct Stream Consumption (`livekit/bridge.rs`)**:
    *   Fixed a bug where only the first chunk was consumed on flush. Audio stream loops now use `while let Some(chunk) = buffer.pop_chunk(...)` ensuring all buffered chunks are safely and iteratively dispatched.
4.  **Legacy Backwards Compatibility**:
    *   Preserved existing `flush()`, `flush_remaining()`, and `process_and_clear()` APIs which dynamically bridge `BytesMut` conversions to `Vec<i16>` for legacy callers.
    *   `AudioFormat` implementations are entirely untouched to prevent downstream impacts.

## Benchmark Results

```text
📊 BENCHMARK RESULTS (1000000 total frame pushes)
Simulating 100 connections, 10000 frames each (10ms frame, 40ms buffer)
------------------------------------------------------------
 Metric                │ Old (flush)       │ New (pop_chunk)         │ Improvement
───────────────────────┼───────────────────┼─────────────────────────┼──────────────
 Total Allocations     │           2500000 │                       0 │ -100.0%
 Total Memory Allocated│      2400000000 B │                     0 B │ -100.0%
 Mean Latency          │            126.74 ns│                73.55 ns│ 1.72x faster
 Median (P50) Latency  │                60 ns│                   44 ns│ 1.36x faster
 P95 Latency           │               137 ns│                   82 ns│ 1.67x faster
 P99 Latency           │              2677 ns│                  397 ns│ 6.74x faster
 Total Wall-Clock Time │            140.24 ms│                88.08 ms│ 1.59x faster
------------------------------------------------------------
```
*Note: Results show O(1) time complexity per buffer dump loop compared to older iterative bounds.*
