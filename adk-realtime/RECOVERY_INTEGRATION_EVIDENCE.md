# Recovery Integration Evidence

Evidence recorded by a downstream integrator (a live telephony data plane)
while porting from a hand-rolled reconnect loop onto the managed recovery
layer, 2026-08-26, against `9542f0afc`.

[`MANAGED_RECOVERY.md`](MANAGED_RECOVERY.md) is the authority for what the
contract *is*; nothing here restates it. This page records only what was
independently **checked**, how, and what remained unchecked — so that the claims
in that document's "Claims that require provider/runtime proof" section can be
narrowed by measurement rather than by assertion.

Every claim below names a file and a line, or a command. Where a claim could not
be verified, it says so rather than rounding up.

## Why an integrator needed to check these

`MANAGED_RECOVERY.md` says the managed layer "can reject new writes safely
during recovery". A caller-audio path needs three sharper facts before it can
delete its own reconnect machinery:

1. whether a refused write **blocks or returns**, because a blocking write in a
   `select!` loop starves hangup detection;
2. whether the refusal is **typed**, because deciding on error text is not
   classification;
3. whether the generation watch fires **after** the session is writable, because
   a buffered-audio drain that races the un-gate replays into a session that
   still refuses.

## Verified

### The write gate is non-blocking and typed

`Supervisor::admit_write` (`src/recovery/supervisor.rs:616`) matches on
`ManagedState` and returns immediately in every arm — there is no `.await` on a
recovery future inside it. The three outcomes map onto exactly the states an
ingress gate needs:

| `ManagedState` | `admit_write` returns |
|---|---|
| `Serving` + `is_writable()` | `Ok(SessionGeneration)` |
| `Recovering { .. }` / `Uninitialized` | `Err(RealtimeError::NotConnected)` |
| `Terminal { ExplicitClose }` | `Err(RealtimeError::SessionClosed)` |
| `Terminal { Exhausted }` | `Err(provider("session exhausted"))` |

`RealtimeRunner::invoke_write` (`src/runner.rs:416`) wraps a gate refusal as
`WriteFailed { certainty: NotAttempted }` and an operation failure as
`WriteFailed { certainty: Indeterminate }`, so a caller can separate "never
sent, safe to hold" from "may have been delivered" using
`RealtimeError::delivery_certainty` (`src/error.rs:229`) and the inner variant,
without inspecting any message text.

**Consequence for integrators.** This removes a check-then-act window that a
polled state cannot: sampling availability and *then* writing leaves a gap in
which the session can die, and frames sent in that gap are lost while the
sampled state still reads healthy. That defect was live in this integrator's
bridge and had been patched at one call site the day before the port.

### Generation publication happens after the session is writable

`src/recovery/supervisor.rs:555-556`, in order:

```rust
*state = ManagedState::Serving { active: Arc::clone(&gen_item), planned: None };
let _ = self.generation_tx.send(gen_id);
```

The state becomes `Serving` **before** the watch fires. A subscriber woken by
`subscribe_generation()` therefore cannot observe a generation whose writes are
still refused, so a replay loop keyed on that watch cannot race the un-gate.

This was checked in code rather than inferred from the doc comment on
`subscribe_generation` (`src/runner.rs:531`), which states the intent but not
the ordering.

### The recovery budget is a total, and it is enforced

`RecoveryPolicy::default()` (`src/recovery.rs:150-156`): `max_attempts = 3`,
`deadline = 5s`, `initial_delay = 50ms`, `max_delay = 500ms`. The deadline is
documented as *total* (`:146`) and applied to the whole episode (`:132`), and
`GeminiRealtimeSession::recover` enforces it with
`tokio::time::timeout_at(Instant::from_std(deadline), attempt_fut)`.

**Consequence for integrators.** A telephony carrier tears down a leg on RTP
inactivity in roughly 10–15s on Elastic SIP trunking, so a *total* 5s budget
fits inside that window with margin. A per-attempt budget of the same number
would not.

### `goAway` produces a real replacement, not just a warning

`RealtimeRunner::next_event` intercepts `ServerEvent::PlannedRotation`
(`src/runner.rs:824-827`), calls `handle_planned_rotation`, and forwards the
event. The rotation is make-before-break: the replacement is established while
the current session is still serving.

Verified end to end from the integrator's side with a mock model scripting a
`goAway` frame, asserting that a **new generation is published** and the session
remains connected. The assertion was falsified with a control — the same test
against a model that scripts no `goAway` fails with:

```
a goAway must produce a replacement generation, not merely a warning
```

so the test detects the rotation specifically rather than passing on ambient
behaviour.

**Why this mattered here.** The integrator's previous pin treated `goAway` as a
documented pre-close warning and deliberately did not act on it. A long call
therefore ran until the provider actually closed the socket and then recovered
*reactively*, with the session already gone. This is the difference between a
planned replacement and a gap.

### A pre-setup close is classified as fatal

`GeminiRealtimeSession::classify` (`src/gemini/session.rs:1501`) returns `Fatal` for
`UnexpectedEof` when the connection never reached `setupComplete` **and** a
close frame was recorded. Retrying there would re-send the setup the server just
refused, and would replace the server's stated reason with a generic
candidate-failure message. Pinned by
`a_server_close_before_setup_complete_is_fatal` and
`a_bare_eof_before_setup_complete_stays_recoverable`, the latter confirmed to
fail if the close-evidence half of the condition is removed.

## Not verified

Stated plainly, because these are the claims `MANAGED_RECOVERY.md` already warns
against making, and this page does not license any of them:

- **No real provider socket was involved.** Every result above comes from mock
  sessions and mock models. The Gemini recovery implementation being *present*
  and *unit-tested* is not evidence that Gemini's live behaviour matches it.
- **No live call was placed.** No measurement exists of what a caller hears
  during a rotation or a recovery — whether audio is continuous, and for how
  long ingress is held.
- **No recovery latency was measured.** The 5s deadline is a bound the code
  enforces, not an observed distribution.
- **The flapping case is bounded only downstream.** `max_attempts` bounds one
  episode; nothing in this layer bounds how many episodes a flapping provider
  can start, each individually inside budget. An integrator that needs a
  per-call ceiling has to keep its own counter — this one re-anchored its cap on
  generation publication.
- **`classify` still routes through `is_connection_reset`**, which substring
  matches `ConnectionError(msg)` (`src/error.rs:255`). Typed variants cover
  `IoError` and `TransportReset`; the string path remains for the rest.

## Reproducing

From the repository root:

```
cargo nextest run -p adk-realtime --features gemini
cargo clippy -p adk-realtime --features gemini --all-targets -- -D warnings
```

`gemini` is **not** a default feature. A bare `cargo test -p adk-realtime --lib`
compiles none of the Gemini surface and reports success having run none of it;
`cargo nextest list --workspace | grep -c gemini::session` returned `0` against
`70` with the feature enabled, which is why the CI matrix carries an explicit
`{ package: adk-realtime, features: gemini }` entry.
