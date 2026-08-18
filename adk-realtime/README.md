# adk-realtime

Real-time bidirectional audio streaming and managed session lifecycle for Rust Agent Development Kit (ADK-Rust) agents.

[![Crates.io](https://img.shields.io/crates/v/adk-realtime.svg)](https://crates.io/crates/adk-realtime)
[![Documentation](https://docs.rs/adk-realtime/badge.svg)](https://docs.rs/adk-realtime)
[![License](https://img.shields.io/crates/l/adk-realtime.svg)](LICENSE)

## Overview

`adk-realtime` provides a unified interface for building voice-enabled AI agents using real-time streaming APIs. It supports provider-specific raw sessions together with a managed `RealtimeRunner` lifecycle for application-facing voice sessions.

The crate follows the **OpenAI Agents SDK pattern** with a separate, decoupled implementation that integrates with the ADK agent ecosystem.

## Features

- **RealtimeAgent** — Implements `adk_core::Agent` with full callback/tool/instruction support
- **Multiple Providers** — OpenAI Realtime API, Gemini Live API, Vertex AI Live API
- **Multiple Transports** — WebSocket, WebRTC (OpenAI), LiveKit bridge
- **Audio Streaming** — Bidirectional audio with PCM16, G711, Opus formats
- **Voice Activity Detection** — Server-side VAD for natural conversation flow
- **Tool Calling** — Real-time function/tool execution during voice conversations
- **Agent Handoff** — Transfer between agents using `sub_agents`
- **Context Mutation** — Swap instructions and tools mid-session (OpenAI: in-place update, providers that require it: managed resumption)
- **Managed Recovery Orchestration** — Generation-fenced recovery, bounded retries, atomic ready-session replacement, and deterministic terminal state
- **Delivery Certainty** — Managed write failures distinguish `NotAttempted` from `Indeterminate` so applications can make safer replay decisions
- **Generation-Safe Managed Reads** — `RealtimeRunner::next_event()` wakes on generation publication without waiting for an old session to close
- **Zero-Allocation LiveKit Audio** — `bytemuck` zero-copy PCM path for LiveKit output
- **Interruption Detection** — `Manual` or `Automatic` VAD-based interruption handling
- **Feature Flags** — Pay only for what you use; all transports are opt-in

## Architecture

```text
              ┌─────────────────────────────────────────┐
              │              Agent Trait                 │
              │  (name, description, run, sub_agents)    │
              └────────────────┬────────────────────────┘
                               │
       ┌───────────────────────┼───────────────────────┐
       │                       │                       │
┌──────▼──────┐      ┌─────────▼─────────┐   ┌─────────▼─────────┐
│  LlmAgent   │      │  RealtimeAgent    │   │  SequentialAgent  │
│ (text-based)│      │  (voice-based)    │   │   (workflow)      │
└─────────────┘      └───────────────────┘   └───────────────────┘
```

### Raw provider sessions and managed lifecycle

```text
RealtimeSession
  raw provider I/O
  fail-fast transport API
  optional RealtimeRecovery capability
        │
        ▼
RealtimeRunner
  managed session generation authority
  write admission + delivery certainty
  recovery/resumption serialization
  config revision fencing
  terminal lifecycle
```

The split is intentional. Raw `RealtimeSession` APIs remain useful when an application wants direct provider control. `RealtimeRunner` is the managed abstraction for applications that want lifecycle and recovery semantics.

### Transport Layer

```text
┌──────────────────────────────────────────────────────────────┐
│                    RealtimeSession trait                     │
├──────────────┬──────────────┬──────────────┬─────────────────┤
│ OpenAI WS    │ OpenAI WebRTC│ Gemini Live  │ Vertex AI Live  │
│ (openai)     │ (openai-     │ (gemini)     │ (vertex-live)   │
│              │  webrtc)     │              │                 │
└──────────────┴──────────────┴──────────────┴─────────────────┘

┌──────────────────────────────────────────────────────────────┐
│              LiveKit WebRTC Bridge (livekit)                 │
│  LiveKitEventHandler · bridge_input · bridge_gemini_input    │
└──────────────────────────────────────────────────────────────┘
```

## Supported Providers & Transports

| Provider | Model | Transport | Feature Flag | Description |
|----------|-------|-----------|--------------|-------------|
| OpenAI | `gpt-realtime` | WebSocket | `openai` | OpenAI realtime transport |
| OpenAI | `gpt-4o-realtime-*` | WebRTC | `openai-webrtc` | Browser-grade transport with Opus codec |
| Google | Gemini Live | WebSocket | `gemini` | Gemini Live API |
| Google | Gemini via Vertex AI | WebSocket + OAuth2 | `vertex-live` | Vertex AI Live with ADC authentication |
| LiveKit | Any supported realtime model (bridge) | WebRTC | `livekit` | Production WebRTC bridge to provider sessions |

## Quick Start

Add to your `Cargo.toml`:

```toml
[dependencies]
adk-realtime = { version = "3.0.0", features = ["openai"] }
```

### Using RealtimeAgent (Recommended)

```rust
use adk_realtime::{RealtimeAgent, openai::OpenAIRealtimeModel};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("OPENAI_API_KEY")?;
    let model = Arc::new(OpenAIRealtimeModel::new(&api_key, "gpt-realtime"));

    let agent = RealtimeAgent::builder("voice_assistant")
        .model(model)
        .instruction("You are a helpful voice assistant.")
        .voice("alloy")
        .server_vad()
        .build()?;

    // RealtimeAgent implements the Agent trait — use with ADK runner
    Ok(())
}
```

### Using Low-Level Session API

Use the raw session API when you deliberately want fail-fast provider control and own reconnect/replay policy yourself:

```rust
use adk_realtime::{RealtimeModel, RealtimeConfig, ServerEvent};
use adk_realtime::openai::OpenAIRealtimeModel;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = OpenAIRealtimeModel::new(
        std::env::var("OPENAI_API_KEY")?,
        "gpt-realtime",
    );

    let config = RealtimeConfig::default()
        .with_instruction("You are a helpful voice assistant.")
        .with_voice("alloy");

    let session = model.connect(config).await?;
    session.send_text("Hello!").await?;
    session.create_response().await?;

    while let Some(event) = session.next_event().await {
        match event? {
            ServerEvent::AudioDelta { delta, .. } => { /* play audio */ }
            ServerEvent::TextDelta { delta, .. } => print!("{}", delta),
            _ => {}
        }
    }
    Ok(())
}
```

## Managed Recovery & Session Continuity

The managed recovery layer is designed for long-lived realtime sessions where a transient transport failure should not create competing reconnect loops or ambiguous session ownership.

A managed recovery episode follows this shape:

```text
operation on generation N
        │
        ▼
transport read/write/EOF failure
        │
        ▼
Recovering(N, epoch E)
        │
        ├─ new managed writes rejected before raw invocation
        │    → DeliveryCertainty::NotAttempted
        │
        ▼
provider builds one private candidate attempt
        │
        ▼
candidate reaches provider readiness boundary
        │
        ▼
config revision revalidated
        │
        ▼
atomically publish generation N+1
        │
        ▼
notify readers → asynchronously retire N
```

### What the managed layer guarantees

- one writable active session + generation authority
- failures are reported against the exact generation that performed the operation
- new writes are not admitted while the managed transport is `Recovering`
- recovery attempts are bounded by policy and a whole-episode deadline
- intentional context resumption and network recovery cannot independently publish competing replacements
- a ready candidate cannot publish if its configuration revision is stale
- generation publication wakes managed readers without waiting for old-session shutdown
- unpublished ready candidates are cleaned up if recovery is cancelled before publication
- `Closed` and `Exhausted` are deterministic non-admittable terminal states

### Provider capability boundary

Managed recovery does **not** mean every provider automatically reconnects today.

`RealtimeSession::recovery()` returns `None` by default. A provider participates in automatic managed recovery only when its concrete session implements the `RealtimeRecovery` SPI and returns a fully ready `RecoveredSession`.

One `RealtimeRecovery::recover()` call represents exactly one provider candidate attempt. The provider owns authentication, transport setup, required provider setup frames, readiness detection, and truthful continuity classification. The supervisor owns retries, deadlines, generation publication, and terminal state.

### Continuity

`RecoveryContinuity` distinguishes:

- **`Resumed`** — provider-native logical session continuity was actually confirmed
- **`Reconnected`** — a new ready transport exists with the current effective config, but previous provider history is not guaranteed

Do not interpret a successful reconnect as proof that provider-side conversation history survived.

### Delivery certainty and replay

A managed `RealtimeError::WriteFailed` can carry:

- **`NotAttempted`** — the managed runner rejected the write before invoking the raw provider session
- **`Indeterminate`** — the raw provider session was invoked, but remote acceptance/processing cannot be proven

`Indeterminate` must not be blindly replayed when duplicate side effects would be harmful.

Recovery is not application replay. Caller audio, business commands, and other domain events remain application-owned.

See **[MANAGED_RECOVERY.md](MANAGED_RECOVERY.md)** for the complete maintenance contract, provider extension checklist, race invariants, test expectations, and supported product claims.

## Operational capability claims

The current managed layer supports precise commercial descriptions such as:

- **managed realtime recovery orchestration**
- **generation-safe session replacement**
- **bounded recovery retries and deadlines**
- **delivery-certainty-aware managed writes**
- **config-safe recovery and context resumption**
- **cancellation-safe unpublished-candidate cleanup**
- **deterministic managed terminal states**

Those are properties of the provider-neutral managed layer and its tests.

Do not infer broader runtime guarantees from them. Claims such as “zero dropped calls”, “exactly-once delivery”, “all providers reconnect automatically”, or provider-specific mid-call recovery require the corresponding provider implementation and real endpoint validation.

A concise truthful description is:

> `adk-realtime` provides a generation-safe managed session layer that can bound recovery work, reject new writes safely while recovering, atomically publish ready replacement sessions, and distinguish writes that were never attempted from writes whose provider outcome is indeterminate.

## Transport Guides

### Vertex AI Live

Connect to Gemini Live API via Vertex AI with Application Default Credentials:

```toml
adk-realtime = { version = "3.0.0", features = ["vertex-live"] }
```

```rust
use adk_realtime::gemini::{GeminiLiveBackend, GeminiRealtimeModel};

// Convenience constructor — auto-discovers ADC credentials
let backend = GeminiLiveBackend::vertex_adc("my-project", "us-central1")?;

// Or manual credentials construction
let credentials = google_cloud_auth::credentials::Credentials::default().await?;
let backend = GeminiLiveBackend::Vertex {
    credentials,
    region: "us-central1".into(),
    project_id: std::env::var("GOOGLE_CLOUD_PROJECT")?,
};

let model = GeminiRealtimeModel::new(backend, "models/gemini-live-2.5-flash-native-audio");
let session = model.connect(config).await?;
```

Prerequisites:
- Google Cloud project with Vertex AI API enabled
- ADC configured (`gcloud auth application-default login`)

### OpenAI WebRTC

Lower-latency audio transport using Sans-IO WebRTC with Opus codec:

```toml
adk-realtime = { version = "3.0.0", features = ["openai-webrtc"] }
```

```rust
use adk_realtime::openai::{OpenAIRealtimeModel, OpenAITransport};

let model = OpenAIRealtimeModel::new(api_key, "gpt-realtime")
    .with_transport(OpenAITransport::WebRTC);
let session = model.connect(config).await?;
```

Build requirement: `cmake` must be installed (the `audiopus` crate builds the Opus C library from source). With cmake >= 4.0, set the environment variable:

```bash
export CMAKE_POLICY_VERSION_MINIMUM=3.5
```

### LiveKit WebRTC Bridge

Bridge any `EventHandler` to a LiveKit room for production voice apps:

```toml
adk-realtime = { version = "3.0.0", features = ["livekit", "openai"] }
```

#### LiveKitConfig and LiveKitRoomBuilder

Use the typestate builder for secure room connections. Credentials are stored with `secrecy::SecretString` and redacted in Debug output:

```rust
use adk_realtime::livekit::{LiveKitConfig, LiveKitRoomBuilder};

let config = LiveKitConfig::new(
    "wss://your-server.livekit.cloud",
    std::env::var("LIVEKIT_API_KEY")?,
    std::env::var("LIVEKIT_API_SECRET")?,
)?;

let bundle = LiveKitRoomBuilder::new(config)
    .identity("my-agent")           // required — enables connect()
    .name("Voice Agent")            // optional display name
    .room_name("session-room-123")  // optional — auto-generated if omitted
    .with_audio(24_000, 1)          // publish a local audio track
    .connect()
    .await?;
```

#### Bridging Audio

```rust
use adk_realtime::livekit::{LiveKitEventHandler, bridge_input};

// Wrap your event handler to publish model audio to LiveKit
let lk_handler = LiveKitEventHandler::new(inner_handler, audio_source, 24000, 1);

// Bridge participant audio from LiveKit into the RealtimeRunner
tokio::spawn(bridge_input(remote_track, runner));
```

For Gemini's 16 kHz format, use `bridge_gemini_input` instead.

## Mid-Session Context Mutation

Swap instructions and tools during a live voice session:

```rust
use adk_realtime::config::{SessionUpdateConfig, RealtimeConfig};

let update = SessionUpdateConfig(
    RealtimeConfig::default()
        .with_instruction("You are now a billing specialist.")
);

runner.update_session(update).await?;
```

The runner handles provider differences through `ContextMutationOutcome`:

- **`Applied`** — provider updated the active session in place
- **`RequiresResumption`** — the managed runner serializes intentional resumption through the same replacement authority used by recovery

See `SESSION_MANAGEMENT.md` for the full context-mutation architecture and `examples/openai_session_update.rs` / `examples/gemini_context_mutation.rs` for examples.

## Interruption Detection

Control how VAD handles user interruptions:

```rust
use adk_realtime::InterruptionDetection;

let config = RealtimeConfig::default()
    .with_interruption_detection(InterruptionDetection::Automatic);
```

- `Manual` (default): Application calls cancellation explicitly
- `Automatic`: VAD detects user speech onset and cancels agent audio output

## Feature Flags

| Flag | Dependencies | Description |
|------|-------------|-------------|
| `openai` | `async-openai`, `tokio-tungstenite` | OpenAI Realtime API (WebSocket) |
| `gemini` | `tokio-tungstenite`, `adk-gemini` | Gemini Live API (AI Studio) |
| `vertex-live` | `gemini` + `google-cloud-auth` | Vertex AI Live API (OAuth2/ADC) |
| `livekit` | `livekit`, `livekit-api` | LiveKit WebRTC bridge |
| `openai-webrtc` | `openai` + `str0m`, `audiopus`, `reqwest` | OpenAI WebRTC transport (requires cmake) |
| `full` | all of the above except openai-webrtc | Everything that doesn't require cmake |
| `full-webrtc` | `full` + `openai-webrtc` | Everything including WebRTC (requires cmake) |

Default features: none. You opt in to exactly what you need.

### Feature Flag Dependency Graph

```text
vertex-live   ──► gemini + google-cloud-auth
openai-webrtc ──► openai + str0m + audiopus + reqwest
livekit       ──► livekit + livekit-api
full          ──► openai + gemini + vertex-live + livekit
full-webrtc   ──► full + openai-webrtc
```

## RealtimeAgent Features

### Shared with LlmAgent

| Feature | Description |
|---------|-------------|
| `instruction(str)` | Static system instruction |
| `instruction_provider(fn)` | Dynamic instruction based on context |
| `global_instruction(str)` | Global instruction (prepended) |
| `tool(Arc<dyn Tool>)` | Register a tool |
| `sub_agent(Arc<dyn Agent>)` | Register sub-agent for handoffs |
| `before_agent_callback` | Called before agent runs |
| `after_agent_callback` | Called after agent completes |
| `before_tool_callback` | Called before tool execution |
| `after_tool_callback` | Called after tool execution |

### Realtime-Specific

| Feature | Description |
|---------|-------------|
| `voice(str)` | Voice selection ("alloy", "coral", "sage", etc.) |
| `server_vad()` | Enable server-side VAD with defaults |
| `vad(VadConfig)` | Custom VAD configuration |
| `modalities(vec)` | Output modalities (["text", "audio"]) |
| `on_audio(callback)` | Callback for audio output events |
| `on_transcript(callback)` | Callback for transcript events |
| `on_speech_started(callback)` | Callback when speech detected |
| `on_speech_stopped(callback)` | Callback when speech ends |

## Event Types

### Server Events

| Event | Description |
|-------|-------------|
| `SessionCreated` | Connection established |
| `AudioDelta` | Audio chunk (base64 PCM or Opus) |
| `TextDelta` | Text response chunk |
| `TranscriptDelta` | Input audio transcript |
| `FunctionCallDone` | Tool call request |
| `ResponseDone` | Response completed |
| `SpeechStarted` | VAD detected speech |
| `SpeechStopped` | VAD detected silence |
| `Error` | Error occurred |

### Client Events

| Event | Description |
|-------|-------------|
| `AudioAppend` | Send audio chunk |
| `AudioCommit` | Commit audio buffer |
| `ItemCreate` | Send text or tool response |
| `ResponseCreate` | Request a response |
| `ResponseCancel` | Interrupt response |
| `SessionUpdate` | Update configuration |

## Audio Formats

| Format | Sample Rate | Bits | Channels | Provider |
|--------|-------------|------|----------|----------|
| PCM16 | 24000 Hz | 16 | Mono | OpenAI |
| PCM16 | 16000 Hz | 16 | Mono | Gemini (input) |
| PCM16 | 24000 Hz | 16 | Mono | Gemini (output) |
| Opus | 24000 Hz | — | Mono | OpenAI WebRTC |
| G711 u-law | 8000 Hz | 8 | Mono | OpenAI |
| G711 A-law | 8000 Hz | 8 | Mono | OpenAI |

## Error Types

Transport-specific errors carry their normal provider/transport context. Managed write failures additionally carry delivery certainty.

| Variant | Feature | Description |
|---------|---------|-------------|
| `WriteFailed` | all managed runner writes | Wraps a failed managed write with `NotAttempted` or `Indeterminate` delivery certainty |
| `OpusCodecError` | `openai-webrtc` | Opus encoding/decoding failures |
| `WebRTCError` | `openai-webrtc` | WebRTC connection and signaling failures |
| `LiveKitError` | `livekit` | LiveKit bridge failures |
| `AuthError` | `vertex-live` | OAuth2/ADC credential failures |
| `ConfigError` | all | Missing or invalid configuration |
| `ConnectionError` | all | Transport connection failures |

For retry-sensitive managed writes, inspect `RealtimeError::delivery_certainty()` rather than assuming every connection error is replay-safe.

## Examples

```bash
# Vertex AI Live voice assistant (requires ADC + GCP project)
cargo run --example vertex_live_voice --features vertex-live

# LiveKit bridge with OpenAI model (requires LiveKit server)
cargo run --example livekit_bridge --features "livekit,openai"

# OpenAI WebRTC low-latency session (requires cmake + API key)
CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo run --example openai_webrtc --features openai-webrtc

# Mid-session context mutation — OpenAI (swap persona + tools)
cargo run --example openai_session_update --features openai

# Mid-session context mutation — Gemini (session resumption)
cargo run --example gemini_context_mutation --features gemini
```

## Testing

```bash
# Workspace-quality gates used by managed recovery changes
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
cargo clippy -p adk-realtime --features integration --all-targets -- -D warnings
cargo nextest run -p adk-realtime --features integration

# Focused property / transport tests (no credentials unless noted)
cargo test -p adk-realtime --test error_context_tests
cargo test -p adk-realtime --features vertex-live --test vertex_url_property_tests
cargo test -p adk-realtime --features livekit --test livekit_delegation_tests
CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo test -p adk-realtime --features openai-webrtc --test opus_roundtrip_tests
CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo test -p adk-realtime --features openai-webrtc --test sdp_offer_tests

# Integration tests that require real credentials are marked #[ignore]
cargo test -p adk-realtime --features vertex-live -- --ignored
```

The managed recovery integration suite proves the provider-neutral state machine and concurrency boundaries. It does not substitute for a real endpoint interruption test when making provider-specific reconnect claims.

## Compilation Verification

```bash
cargo check -p adk-realtime                          # default (no deps)
cargo check -p adk-realtime --features openai        # OpenAI WebSocket
cargo check -p adk-realtime --features gemini        # Gemini Live
cargo check -p adk-realtime --features vertex-live   # Vertex AI Live
cargo check -p adk-realtime --features livekit       # LiveKit bridge
CMAKE_POLICY_VERSION_MINIMUM=3.5 \
  cargo check -p adk-realtime --features openai-webrtc  # OpenAI WebRTC
CMAKE_POLICY_VERSION_MINIMUM=3.5 \
  cargo check -p adk-realtime --features full            # everything
```

## Maintenance

Contributors touching session lifecycle, recovery, context resumption, write certainty, or generation management should read:

- **[MANAGED_RECOVERY.md](MANAGED_RECOVERY.md)** — managed recovery architecture, invariants, provider SPI, testing, and product-claim boundaries
- **[SESSION_MANAGEMENT.md](SESSION_MANAGEMENT.md)** — mid-session context mutation and resumption architecture
- **[AGENTS.md](AGENTS.md)** — crate-local concurrency, audio, and recovery maintenance rules

The most important recovery rule is simple: **one active session-generation authority, one replacement/publication authority, and no application replay hidden inside the generic recovery layer.**

## License

Apache-2.0

## Part of ADK-Rust

This crate is part of the [ADK-Rust](https://github.com/zavora-ai/adk-rust) framework for building AI agents in Rust.
