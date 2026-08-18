# Realtime Concurrency, Audio, and Recovery Rules

## Context

This repository contains latency-sensitive realtime voice paths built on:

- async session orchestration in `adk-realtime`
- LiveKit / WebRTC audio bridging
- bidirectional WebSocket transports such as Gemini
- short, per-frame audio hot paths

You MUST optimize for low lock contention, short critical sections, and clear ownership of I/O resources.

---

## 1. Choose the mutex by runtime behavior

### `tokio::sync::{Mutex, RwLock}`
Use for async orchestration state and true cross-task async coordination.

- You MUST NOT use Tokio locks for short CPU-bound hot paths.
- You MUST NOT keep Tokio lock guards alive across provider or network `.await` calls when a handle can be cloned first.
- You MUST prefer the session-generation snapshot pattern in `RealtimeRunner`: acquire authority → clone the exact generation/session handle → drop guards → await provider I/O.

**References**
- Tokio documents that the async mutex is **more expensive** than the blocking mutex and that the main use case is the ability to keep the guard across `.await`; for plain data, `std::sync::Mutex` is often preferred, and `parking_lot` is also called out as a good fit. [^tokio-mutex]
- Tokio `RwLock` is appropriate for async shared state coordination, but the same “do not hold guards longer than necessary” principle still applies. [^tokio-rwlock]

### `parking_lot::Mutex`
Use for short, synchronous, CPU-bound hot paths that never cross `.await`.

Good fits:
- Opus encoder access
- short audio-buffer mutation paths
- other high-frequency sync-only sections

Rules:
- You MUST keep the locked scope tiny.
- You MUST drop the guard before any `.await`.
- You MUST NOT use it as a substitute for async orchestration locks.

**References**
- `parking_lot::Mutex` is a blocking mutex with **eventual fairness** and **no poisoning**; it is therefore a good fit for short sync critical sections, not for async state that must survive across `.await`. [^parking-lot-mutex]

### `std::sync::Mutex`
Use as the default sync mutex for small internal state that does not cross `.await`.

Good fits:
- local bookkeeping
- small shared flags, counters, or state
- places where poisoning is acceptable

Rules:
- You MUST drop the guard before any `.await`.
- You MUST ONLY switch to Tokio mutexes when async lifetime is genuinely required.

**References**
- The standard mutex is the baseline blocking mutex and includes poisoning semantics after panic. [^std-mutex]
- Tokio explicitly says the blocking mutex is often preferred for plain data. [^tokio-mutex]

---

## 2. Never hold session locks across `.await`

In `RealtimeRunner` helpers such as `send_audio`, `send_text`, `commit_audio`, `create_response`, `interrupt`, and `next_event`:

- acquire the supervisor/session-generation authority
- clone the exact generation/session snapshot used by the operation
- drop the guard
- then perform the async call

You MUST NOT await provider I/O while still holding a general state lock guard.

A dedicated serialization lock such as the private replacement/publication lock MAY be intentionally held across candidate-producing provider I/O when that lock is the explicit single-flight authority for the transaction. Do not generalize that exception to ordinary reads or writes.

**References**
- This follows Tokio’s guidance that async mutexes are primarily for cases where the guard must cross `.await`; if you do not need that, prefer shorter lock lifetimes and cheaper blocking primitives where possible. [^tokio-mutex]

---

## 3. Use a single writer for WebSocket sinks

For bidirectional WebSocket sessions:

- one dedicated `writer_task` MUST own the sink
- all outbound messages MUST go through a bounded `tokio::sync::mpsc`
- `close()` MUST send `Message::Close(...)` through that channel and await writer shutdown

You MUST NOT allow multiple methods to write directly to the sink through a shared mutex.

**References**
- **PROJECT RULE:** this is an architectural rule for this repository, not a literal sentence from one upstream doc.
- It is informed by the `Sink` model, which requires mutable access to send items, and by Tokio’s bounded `mpsc` model for coordinated async message passing and backpressure. [^futures-sink] [^tokio-mpsc]

---

## 4. Treat audio paths as hot paths

- You MUST keep critical sections short.
- You MUST extract buffered data under a short sync lock, then perform async work after the lock is released.
- You SHOULD prefer `bytes::Bytes` / `bytes::BytesMut` in high-frequency buffering paths when they reduce copies or reallocations.
- You MUST avoid casual `Vec<u8>` use in the hottest paths when `Bytes` would be a better fit.

**References**
- `Bytes` is designed for cheap cloning and shared byte storage; `BytesMut` is the mutable companion for efficient incremental buffer building. [^bytes] [^bytesmut]
- **PROJECT RULE:** “avoid casual `Vec<u8>` in the hottest paths” is a repo policy derived from realtime latency goals, not a blanket prohibition from the `bytes` crate docs.

---

## 5. Do not add `block_in_place` around lightweight LiveKit FFI by default

You MUST NOT wrap lightweight calls such as `NativeAudioStream::new(...)` in `tokio::task::block_in_place(...)` unless profiling proves they are meaningful blockers.

Why:
- it is risky on `current_thread` runtimes
- it increases test/runtime fragility
- it is not the default fix for FFI boundaries

If isolation is required and lifetimes allow it, you SHOULD prefer `spawn_blocking`.

**References**
- Tokio documents that `block_in_place` cannot be used on the `current_thread` runtime and is intended for blocking operations that cannot be avoided. [^tokio-block-in-place]
- Tokio documents `spawn_blocking` as the standard mechanism for offloading blocking work. [^tokio-spawn-blocking]

---

## 6. Keep buffering conversational unless measurements justify otherwise

You MUST use low-latency buffering for interactive voice paths.

You MUST NOT increase buffering substantially unless profiling shows it is necessary for stability.

**References**
- **PROJECT RULE:** this is a product/latency rule for interactive voice behavior in this repository, not a strict upstream library contract.

---

## 7. Prefer managed lifecycle handling for transient disconnects

For application-facing `RealtimeRunner` paths, temporary transport loss MUST be handled by the managed session-generation lifecycle rather than by ad-hoc polling or a second reconnect loop.

- `RealtimeRunner` is the managed abstraction.
- `RealtimeSession` remains the raw provider abstraction.
- Managed reads MUST be generation-aware and wake on generation publication.
- Managed writes MUST pass through the common write-admission boundary.
- You MUST NOT add a third reconnect authority in provider adapters, event handlers, or application bridges.

A raw provider session may still fail fast. Do not silently change raw APIs into policy-owning managed APIs.

**References**
- **PROJECT RULE:** this is the managed recovery architecture established by the realtime recovery integration. See `MANAGED_RECOVERY.md`.

---

## 8. Scope concurrency changes narrowly

You MUST NOT do workspace-wide mutex substitutions.

Preferred order:
1. fix lock lifetime first
2. optimize lock type second

You MUST keep Tokio locks in async orchestration code, use `parking_lot::Mutex` ONLY for proven sync hot paths, and use `std::sync::Mutex` for small sync-only state.

**References**
- This rule follows the division of responsibility documented by Tokio for async mutexes and by `parking_lot` / `std` for blocking mutexes. [^tokio-mutex] [^parking-lot-mutex] [^std-mutex]
- **PROJECT RULE:** “no workspace-wide substitutions” is a repo policy intended to prevent broad, low-signal lock churn.

---

## 9. Preserve PCM16 channel-frame continuity in audio bridges

Provider PCM16 byte streams arrive in arbitrary chunk sizes that need not align
with sample boundaries or channel-frame boundaries (`num_channels *
size_of::<i16>()` bytes).

- You MUST carry incomplete trailing bytes at **channel-frame** granularity,
  not sample granularity: emitting a partial frame shifts every later sample by
  one channel (stereo phase inversion).
- You MUST NOT drop an entire chunk because its decoded sample count is not a
  multiple of `num_channels`, and you MUST NOT silently truncate trailing
  bytes — both are audible data loss.
- Carried bytes MUST be scoped to a single response `item_id` and cleared at
  item-transition, response-done, response-cancelled (barge-in/interruption —
  a normal boundary, never modeled as an error), and error boundaries, so one
  item's tail can never contaminate the next item's audio.
- Every discard MUST be observable: `tracing::warn!` with the item id, the
  boundary name, and the discarded byte count. Silent audio loss is never
  acceptable degradation.
- Carry state MUST stay bounded below one channel frame, and the aligned
  complete-frame path MUST keep the zero-copy `Cow::Borrowed` cast — allocate
  only when combining a pending carry.

**References**
- **PROJECT RULE:** established by the LiveKit PCM channel-frame continuity
  fix. Reference implementation and exhaustive chunk-boundary tests (every
  chunk size across 1–4 channels): `RemainderState` in
  `src/livekit/handler.rs`.

---

## 10. Preserve the managed recovery authority

`RecoverySupervisor` is the single writable authority for the active realtime session generation, canonical configuration revision, transport status, recovery publication, and terminal lifecycle.

You MUST preserve these invariants:

1. `RealtimeRunner` MUST NOT regain a second synchronized active-session cache.
2. Every managed read or write MUST capture the generation together with the exact raw session it invokes.
3. Failure reporting MUST use that captured generation, never the generation current later during error handling.
4. `set_initial_session()` is valid only while the supervisor is uninitialized; later replacements mint a new monotonic generation.
5. Recovery and intentional context resumption MUST share the same replacement/publication serialization boundary.
6. Generation watchers are liveness signals only; supervisor state remains authoritative.
7. Candidate publication order MUST remain: ready candidate -> revision revalidation -> atomic N+1 publication -> watcher notification -> bounded asynchronous retirement of N.
8. A candidate MUST NOT be published if the coherent config revision changed while it was being built.
9. `Closed` and `Exhausted` MUST be non-admittable terminal states.

You MUST NOT restore `RealtimeAvailability` or invent another public availability/reconnect state machine to compete with the supervisor.

See `MANAGED_RECOVERY.md` for the full transaction and extension contract.

---

## 11. Preserve delivery certainty

`DeliveryCertainty` records only what the managed runner knows about the local provider invocation boundary.

- `NotAttempted` means the managed layer rejected the write before invoking the raw session.
- `Indeterminate` means the raw session was invoked and remote acceptance/processing is not known.

Rules:

- A pre-invocation rejection MUST perform zero raw calls.
- Once the raw session is invoked, a failure MUST NOT be labeled `NotAttempted`.
- You MUST NOT interpret `Indeterminate` as safe to retry automatically.
- You MUST NOT interpret a successful raw write as proof of provider-side business processing.
- Application replay policy MUST stay outside generic ADK recovery.

This boundary is customer-facing reliability semantics, not merely an internal error detail.

---

## 12. Recovery episodes are cancellation-safe transactions

When recovery for generation N begins:

- the supervisor transitions to private `Recovering`
- new managed writes MUST be rejected before raw invocation
- one recovery epoch identifies the active episode
- the cancellation guard MUST own the episode from the moment `Recovering` is published until a replacement or terminal state is published

Cleanup from an older cancelled episode MUST NOT restore `Healthy` during a newer episode.

A ready but unpublished provider candidate MUST remain cleanup-owned until atomic publication. Cancelling before publication MUST leave N authoritative and close the unpublished candidate with bounded cleanup work.

Do not insert an `.await` between publishing `Recovering` and establishing guard ownership. Do not disarm the episode guard before publishing `Healthy` or `Exhausted`.

---

## 13. Provider recovery is one candidate attempt

A provider opts into managed recovery through `RealtimeSession::recovery()` and `RealtimeRecovery`.

One `recover(context)` call MUST mean exactly one provider candidate attempt.

The provider owns:

- provider-specific cause/error classification
- authentication or token refresh for that attempt
- transport creation
- setup/configuration frames
- the provider-specific readiness boundary
- attempt-local cleanup
- truthful `RecoveryContinuity`

The provider MUST NOT own:

- the outer retry loop
- backoff policy
- generation allocation/publication
- application replay
- another session replacement authority

`RecoveredSession` may be returned only after the candidate is actually ready for managed traffic. `Resumed` requires confirmed provider-native logical continuity; otherwise use `Reconnected`.

---

## 14. Do not overclaim runtime reliability

The provider-neutral recovery state machine and its deterministic tests prove the generic managed boundary. They do not prove that a specific external provider can reconnect successfully under real network conditions.

When documenting or marketing this crate:

You MAY claim:

- managed realtime recovery orchestration
- generation-safe session replacement
- bounded recovery attempts/deadlines
- delivery-certainty-aware writes
- config-safe recovery/resumption publication
- cancellation-safe candidate cleanup

You MUST NOT claim without provider/runtime evidence:

- universal automatic provider reconnect
- zero dropped calls
- zero lost audio
- exactly-once provider delivery
- preserved provider history after every reconnect
- a recovery latency SLA

Gemini- or provider-specific runtime claims require the corresponding provider implementation plus real endpoint validation.

---

## 15. Required recovery validation

For changes touching managed recovery, session-generation authority, write certainty, resumption, or terminal lifecycle, run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
cargo clippy -p adk-realtime --features integration --all-targets -- -D warnings
cargo nextest run -p adk-realtime --features integration
```

Tests SHOULD explicitly cover the interleaving or boundary being changed. A test name is not proof unless synchronization actually forces the intended race.

Do not replace deterministic synchronization with fixed sleeps when a notification, barrier, held lock, bounded poll, or paused Tokio clock can prove the state transition directly.

---

[^tokio-mutex]: Tokio `Mutex` docs: <https://docs.rs/tokio/latest/tokio/sync/struct.Mutex.html>
[^tokio-rwlock]: Tokio `RwLock` docs: <https://docs.rs/tokio/latest/tokio/sync/struct.RwLock.html>
[^std-mutex]: Rust standard library `Mutex` docs: <https://doc.rust-lang.org/std/sync/struct.Mutex.html>
[^parking-lot-mutex]: `parking_lot::Mutex` docs: <https://docs.rs/parking_lot/latest/parking_lot/type.Mutex.html>
[^tokio-mpsc]: Tokio `mpsc` docs: <https://docs.rs/tokio/latest/tokio/sync/mpsc/index.html>
[^futures-sink]: `futures::Sink` trait docs: <https://docs.rs/futures/latest/futures/sink/trait.Sink.html>
[^bytes]: `bytes::Bytes` docs: <https://docs.rs/bytes/latest/bytes/struct.Bytes.html>
[^bytesmut]: `bytes::BytesMut` docs: <https://docs.rs/bytes/latest/bytes/struct.BytesMut.html>
[^tokio-block-in-place]: Tokio `block_in_place` docs: <https://docs.rs/tokio/latest/tokio/task/fn.block_in_place.html>
[^tokio-spawn-blocking]: Tokio `spawn_blocking` docs: <https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html>