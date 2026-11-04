# Managed Realtime Recovery

This document describes the provider-neutral managed recovery behavior implemented by `adk-realtime` as of the managed recovery integration merged in PR #231.

It is both a user guide and a maintenance contract. It documents what the crate can truthfully promise today, which layer owns each responsibility, and which behaviors must remain invariant as provider-specific recovery support evolves.

## What is shipped

`RealtimeRunner` now owns a managed session lifecycle built around one private `RecoverySupervisor` authority.

The managed layer provides:

- one authoritative active `SessionGeneration { id, session }`
- monotonic generation IDs across initial connection, recovery, and intentional resumption
- generation-fenced read, write, and EOF failure reporting
- one managed write boundary for side-effectful runner operations
- explicit delivery certainty: `NotAttempted` versus `Indeterminate`
- a private `Recovering` transport phase that rejects new writes before raw provider invocation
- bounded retry policy with a whole-episode deadline
- one replacement/publication lock shared by transport recovery and intentional context resumption
- coherent configuration value + revision snapshots
- stale-candidate rejection when configuration changes during candidate construction
- candidate-ready -> atomic publish -> watcher notification -> asynchronous old-session retirement ordering
- cancellation cleanup for unpublished ready candidates
- generation-watch wakeups for managed `next_event()` readers
- deterministic `Closed` and `Exhausted` terminal states
- continuity reporting through `RecoveryContinuity::{Resumed, Reconnected}`

These behaviors are provider-neutral. They describe how the managed runner supervises recovery once a session exposes a `RealtimeRecovery` capability.

## What is not implied

Managed recovery is not the same as universal provider reconnect support.

A raw `RealtimeSession` returns `None` from `recovery()` by default. Automatic provider recovery is available only when the concrete provider session implements `RealtimeRecovery` and returns a ready `RecoveredSession`.

In particular:

- the generic runner does not manufacture a provider connection by itself
- the generic runner does not replay application audio or business commands
- `Indeterminate` does not mean the provider processed a command and does not mean it did not process it
- `Ok(())` from a raw write does not prove remote business processing
- `Reconnected` does not claim prior provider conversation history survived
- `Resumed` must be used only when provider-native logical continuity was actually confirmed
- application-level replay remains an application responsibility

Provider-specific candidate construction, such as transactional Gemini Live recovery, is a separate layer and must satisfy the SPI documented below before the managed runner can publish the replacement.

## Raw versus managed API

The distinction is intentional:

```text
RealtimeSession
  raw provider session
  fail-fast I/O
  provider-specific capability surface

RealtimeRunner
  managed lifecycle
  generation authority
  write certainty
  recovery/resumption serialization
  terminal state
```

Use raw `RealtimeSession` APIs when the caller deliberately wants direct provider control and owns failure handling.

Use `RealtimeRunner` for application-facing voice sessions that need managed lifecycle semantics.

Do not add automatic reconnect behavior directly to every raw `RealtimeSession` method. Recovery policy belongs to the managed layer.

## Recovery transaction

A successful managed recovery follows this ordering:

```text
operation captures generation N
        |
        v
raw read/write/EOF failure on N
        |
        v
failure report for N
        |
        v
single replacement authority acquired
        |
        v
status = Recovering(epoch E)
        |
        +--> new managed writes are rejected before raw invocation
        |      => DeliveryCertainty::NotAttempted
        |
        v
provider builds one candidate attempt privately
        |
        v
candidate reaches provider-specific readiness boundary
        |
        v
config revision revalidated
        |
        v
atomically publish generation N+1 as Healthy
        |
        v
notify generation watchers
        |
        v
retire N asynchronously with bounded close work
```

If candidate construction or publication fails, generation N remains authoritative until a replacement is actually published or the episode becomes terminal.

## Delivery certainty

`RealtimeError::WriteFailed` records what the managed layer knows about the local invocation boundary.

### `DeliveryCertainty::NotAttempted`

The managed runner rejected the operation before invoking the captured raw session.

Typical examples:

- no active session
- transport is currently `Recovering`
- session is terminally exhausted
- session has been explicitly closed

Application-level buffering or retry can be considered because the provider session was not invoked by that operation.

### `DeliveryCertainty::Indeterminate`

The raw provider session was invoked and returned an error. The managed layer cannot prove whether the peer accepted or processed the payload.

Do not blindly replay a side-effectful command after `Indeterminate`.

For audio, text, or other application data, the application must decide whether replay is semantically safe. ADK intentionally does not guess.

## Provider recovery SPI

A provider opts into managed recovery by returning a `RealtimeRecovery` implementation from `RealtimeSession::recovery()`.

One call to `RealtimeRecovery::recover()` means exactly one candidate attempt.

The provider implementation must:

1. classify the triggering cause as recoverable or fatal
2. build the candidate privately
3. perform provider authentication/refresh needed for that attempt
4. send required setup/configuration before application traffic
5. wait for the provider's readiness boundary
6. honor `RecoveryContext::deadline()`
7. return `RecoveredSession` only after the candidate is ready for managed use
8. report `Resumed` only when provider-native logical continuity is confirmed
9. otherwise report `Reconnected`
10. clean up its own temporary resources if the attempt fails before returning

The provider must not own the outer retry loop, backoff policy, generation publication, or application replay.

## Continuity semantics

`RecoveryContinuity` is deliberately narrow.

### `Resumed`

Use only when the provider confirms that the logical session/conversation continuity survived through a provider-native resume mechanism.

### `Reconnected`

A new ready transport/session exists with the current effective configuration applied, but previous provider history is not guaranteed.

Applications that need durable conversational state should keep that state outside the transient provider transport.

## Configuration authority

The supervisor owns one coherent configuration snapshot:

```text
ConfigSnapshot {
  config,
  revision,
}
```

Every authoritative managed configuration mutation must increment the same revision.

A candidate built from revision R may publish only if R is still current at publication time. A stale candidate is rejected and cleaned up rather than silently replacing a newer configuration.

Do not split config value and revision into separate independently locked authorities.

## Recovery and intentional resumption

Network recovery and intentional context resumption have different causes and retry policies, but they share one replacement/publication serialization boundary.

This prevents two candidate-producing paths from independently racing to replace the active generation.

Do not model intentional context resumption as a fake network `FailureReport`.

## Cancellation and terminal behavior

The managed layer must never remain indefinitely in a synthetic `Recovering` state after its recovery future is cancelled.

A recovery episode is epoch-fenced. Cleanup from an older cancelled episode may restore its own abandoned state only if its epoch is still current; it must never reopen writes during a newer recovery episode.

A ready but unpublished candidate is owned by a cleanup guard until publication. Cancelling the recovery future before publication leaves the old generation authoritative and closes the unpublished candidate with bounded cleanup work.

Explicit close is terminal:

- no active session remains admittable
- later sends invoke the raw session zero times
- later EOF/write failures must not start automatic recovery

Exhaustion is also terminal for that generation and duplicate reports must not launch new attempts.

## Managed reads

`RealtimeRunner::next_event()` is a managed pull API.

It watches the active generation while waiting on the current session. Publication of N+1 wakes a reader blocked on N even when:

- N and N+1 expose the same provider `session_id()`
- closing N hangs

The generation watch is a liveness signal only. The supervisor state remains the authority.

Provider `next_event()` implementations still need to be cancellation-safe: consuming provider data before a cancellable await can lose an event when the managed runner switches generations.

## Application replay boundary

Recovery is not replay.

The managed layer restores a usable provider session. The application owns any buffered caller audio, business commands, or other domain events that may need replay.

A safe application policy can use delivery certainty as one input:

```text
NotAttempted  -> provider was not invoked by this operation
Indeterminate -> provider was invoked; duplicate effect is possible
```

Do not introduce an application-history journal into the generic recovery supervisor.

## Maintenance invariants

Changes to managed recovery must preserve all of the following unless the public contract is deliberately revised:

1. `RecoverySupervisor` is the only writable active session + generation authority.
2. `RealtimeRunner` must not grow a second synchronized session cache.
3. Operations report the generation captured with the exact raw session they invoked.
4. A write rejected before raw invocation is `NotAttempted` and performs zero raw calls.
5. A failed write after raw invocation is `Indeterminate`.
6. `Recovering` rejects new writes before raw invocation.
7. Recovery and intentional resumption share one replacement/publication lock.
8. Candidate readiness precedes publication.
9. N+1 publishes before N is retired.
10. Candidate publication revalidates the coherent config revision.
11. Generation IDs never move backward or restart after initialization.
12. Cancellation cannot leave the supervisor permanently `Recovering`.
13. Cleanup from episode E cannot modify a newer episode E+1.
14. `Closed` and `Exhausted` are non-admittable terminal states.
15. Generation watchers provide liveness, not authority.
16. Application replay stays outside generic ADK recovery.
17. Provider implementations own one candidate attempt, not the outer retry loop.

## Tests that protect the contract

The merged integration tests cover, among other cases:

- single active session authority
- generation monotonicity
- `NotAttempted` versus `Indeterminate`
- raw write failure triggering generation-fenced recovery
- concurrent write rejection while `Recovering`
- stale failure coalescing
- same-session-ID generation wakeup
- hanging old-session close
- config-revision stale-candidate rejection
- candidate-ready cancellation cleanup
- recovery-episode cancellation repair
- stale episode cleanup versus a newer episode
- resumption last-write-wins behavior
- recovered tool-output failure under `run_with_cancellation()`
- explicit close leaving no admittable session
- fatal, timeout, and exhausted terminal diagnostics

Run the repository gates before changing this subsystem:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
cargo clippy -p adk-realtime --features integration --all-targets -- -D warnings
cargo nextest run -p adk-realtime --features integration
```

A passing mock/integration suite proves the provider-neutral state machine and boundaries. It does not prove that a real external provider reconnects successfully.

## Product and sales language

The implementation supports strong but specific claims.

### Claims supported by the code and tests

It is reasonable to describe `adk-realtime` as providing:

- **managed realtime session recovery orchestration**
- **generation-safe session replacement**
- **bounded retry and recovery deadlines**
- **delivery-certainty-aware write failures**
- **config-safe recovery and context resumption**
- **cancellation-safe unpublished-candidate cleanup**
- **managed read/write lifecycle with deterministic terminal states**
- **provider-neutral recovery SPI for realtime backends**

Integration evidence gathered against these claims by a downstream consumer —
what was independently checked, with file and line, and what was not — is
recorded separately in
[`RECOVERY_INTEGRATION_EVIDENCE.md`](RECOVERY_INTEGRATION_EVIDENCE.md).

### Claims that require provider/runtime proof

Do not claim any of the following solely because this managed layer exists:

- "Gemini automatically reconnects mid-call" unless the Gemini provider recovery implementation is present and verified
- "zero dropped calls"
- "zero lost audio"
- "exactly-once command delivery"
- "conversation history is always preserved"
- "all providers recover automatically"
- a specific recovery latency SLA without measured production evidence

A truthful commercial description is:

> `adk-realtime` includes a generation-safe managed recovery layer that can reject new writes safely during recovery, bound retry work, atomically replace ready sessions, and distinguish locally unattempted writes from indeterminate provider writes. Provider-specific recovery capabilities plug into this layer and must be validated against the real provider before claiming end-to-end reconnect behavior.

## Extension checklist

When adding a new provider recovery implementation:

- implement `RealtimeRecovery` on the provider session
- keep `recover()` to one candidate attempt
- classify only genuinely recoverable failures as recoverable
- honor the absolute deadline
- reach the provider's real readiness signal before returning
- apply the effective config before returning
- return the correct continuity value
- ensure candidate cleanup is cancellation-safe
- add deterministic provider-level tests
- add a real endpoint interruption test before making end-to-end reliability claims

When adding application replay:

- keep replay ownership in the application layer
- preserve FIFO order where ordering matters
- use `DeliveryCertainty` rather than assuming every write is safe to replay
- bound buffers and cleanup on call teardown
- separately prove the actual end-to-end call behavior
