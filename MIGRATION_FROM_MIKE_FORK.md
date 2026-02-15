# Migration Guide: mikefaille/adk-rust → zavora-ai/adk-rust

This document helps Mike (and anyone on his fork) migrate to our `feat/realtime-audio-transport` branch. It covers every API difference, the reasoning behind our choices, and exact code changes needed.

## TL;DR

Both implementations provide the same capabilities — OpenAI WebRTC, Gemini Live (Studio + Vertex), and LiveKit bridging. The core traits (`RealtimeSession`, `EventHandler`, `RealtimeRunner`, `RealtimeAgent`) are API-compatible. The differences are in plumbing:

| Area | Mike's Fork | Our Implementation |
|------|------------|-------------------|
| WebRTC crate | `rtc` v0.8.5 | `str0m` v0.16 |
| Feature flags | `webrtc`, `vertex` | `openai-webrtc`, `vertex-live` |
| `GeminiLiveBackend` location | `adk-gemini` | `adk-realtime` |
| LiveKit module | single `livekit.rs` | `livekit/` directory |
| Audio in events | `bytes::Bytes` | `Vec<u8>` |
| SDP signaling | Direct API key | Ephemeral token (OpenAI recommended) |
| Data channel buffering | Pre-open queue in event loop | Pre-open queue in session struct |
| `full` feature | includes WebRTC | excludes WebRTC (no cmake needed) |

---

## Feature Flag Mapping

| Feature | Mike | Ours | Notes |
|---------|------|------|-------|
| OpenAI WebSocket | `openai` | `openai` | Same |
| Gemini Live | `gemini` | `gemini` | Same |
| Vertex AI | `vertex` | `vertex-live` | We use `google-cloud-auth` directly instead of forwarding to `adk-gemini/vertex` |
| LiveKit | `livekit` | `livekit` | Same |
| WebRTC | `webrtc` | `openai-webrtc` | More specific name; both are OpenAI-only |
| All (no cmake) | — | `full` | `openai + gemini + vertex-live + livekit` |
| All (with cmake) | `full` | `full-webrtc` | Adds `openai-webrtc` which requires cmake for `audiopus` |

**Why `full` excludes WebRTC:** The `audiopus` crate builds the Opus C library from source and requires `cmake`. Since `full` is the default feature, including WebRTC would force every user to install cmake even if they don't need WebRTC. Users who want WebRTC opt in with `--features openai-webrtc` or `--features full-webrtc`.

**cmake 4.x note:** With cmake >= 4.0, set `CMAKE_POLICY_VERSION_MINIMUM=3.5` in your environment to work around the bundled Opus CMakeLists.txt.

---

## Detailed Differences

### 1. WebRTC Crate: `rtc` vs `str0m`

Both are Sans-IO WebRTC implementations. Neither requires cmake — that's `audiopus`.

| Aspect | Mike (`rtc` v0.8.5) | Ours (`str0m` v0.16) |
|--------|---------------------|---------------------|
| RTP | Manual `Packet` + `Header` construction | `writer(mid).write(pt, wallclock, rtp_time, data)` |
| SSRC | Manual random generation | Managed internally |
| ICE | Manual `CandidateHostConfig` + gathering loop | Handled by SDP API |
| Data channel | `pc.data_channel(id).send_text()` | `rtc.channel(id).write(reliable, bytes)` |
| SDP | `create_offer()` / `set_remote_description()` | `sdp_api().apply()` / `accept_answer()` |
| I/O loop | `tokio::spawn(run_pc_loop(...))` | Caller-driven via `poll_output()` |

**Migration:** No action needed — our `OpenAIWebRTCSession` is a drop-in replacement that handles all of this internally.

### 2. GeminiLiveBackend Enum

**Mike** (in `adk-gemini/src/types.rs`):
```rust
pub enum GeminiLiveBackend {
    Studio { api_key: String },
    Vertex(VertexContext),           // Pre-authenticated token
    VertexADC { project, location }, // ADC auto-discovery
}
```

**Ours** (in `adk-realtime/src/gemini/session.rs`):
```rust
pub enum GeminiLiveBackend {
    Studio { api_key: String },
    Vertex { credentials, region, project_id },
}

impl GeminiLiveBackend {
    pub fn studio(api_key: impl Into<String>) -> Self;
    pub fn vertex_adc(project_id: impl Into<String>, region: impl Into<String>) -> Result<Self>;
}
```

We collapsed Mike's two Vertex variants into one `Vertex` variant plus a `vertex_adc()` convenience constructor. The constructor auto-discovers ADC credentials, giving the same ergonomics as Mike's `VertexADC` variant.

**Migration:**
```rust
// Before (Mike)
let backend = GeminiLiveBackend::VertexADC { project, location };

// After — convenience constructor (recommended)
let backend = GeminiLiveBackend::vertex_adc(project, location)?;

// After — manual credentials
let credentials = google_cloud_auth::credentials::Builder::default().build()?;
let backend = GeminiLiveBackend::Vertex { credentials, region: location, project_id: project };
```

### 3. Gemini `send_text()` Format

Both implementations use the correct `client_content.turns[]` format with `turn_complete: true`. No migration needed.

### 4. SDP Signaling Flow

**Mike:** Single-step — sends API key directly to the SDP endpoint:
```
POST /v1/realtime?model=...
Authorization: Bearer {api_key}
Content-Type: application/sdp
```

**Ours:** Two-step — ephemeral token first, then SDP exchange:
```
POST /v1/realtime/sessions        → ephemeral token
POST /v1/realtime?model=...       → SDP answer (using ephemeral token)
```

Our approach follows OpenAI's recommended pattern. The ephemeral token is short-lived and scoped, so if it leaks it's less damaging than a full API key. For server-side use where the key never leaves your backend, Mike's approach works fine too.

**Migration:** No action needed — this is internal to the session.

### 5. Data Channel Pre-Open Buffering

Both implementations buffer messages when the data channel isn't open yet:

**Mike:** Buffers in the `run_pc_loop` event loop with `pending_dc_messages: Vec<String>`, flushes on `OnDataChannel(OnOpen)`.

**Ours:** Buffers in the `OpenAIWebRTCSession` struct with `pending_dc_messages: Arc<Mutex<Vec<Vec<u8>>>>`, flushed via `flush_pending_dc_messages()`. Both cap at 50 messages.

**Migration:** No action needed — same behavior, different location.

### 6. Events — Audio Data Type

**Mike:** `bytes::Bytes` for audio, plus `format: AudioFormat` on `ClientEvent::AudioDelta` (skipped in serde).

**Ours:** `Vec<u8>` for audio, plus `format: Option<AudioFormat>` on `ClientEvent::AudioDelta` (skipped in serde).

**Migration:**
```rust
// If passing Bytes to event constructors:
let audio_vec: Vec<u8> = bytes_value.to_vec();

// If reading audio from events:
// delta is Vec<u8> instead of Bytes — same API for slicing/iterating
```

### 7. OpenAI WebRTC Model API

**Mike:** Separate `OpenAiWebRtcModel` struct.

**Ours:** Single `OpenAIRealtimeModel` with transport selection.

**Migration:**
```rust
// Before (Mike)
let model = OpenAiWebRtcModel::new("gpt-4o-realtime-preview", api_key);

// After
let model = OpenAIRealtimeModel::new(api_key, "gpt-4o-realtime-preview")
    .with_transport(OpenAITransport::WebRTC);
```

### 8. LiveKit Module

**Mike:** Single `livekit.rs`, `LiveKitEventHandler::new(source, inner)`.

**Ours:** `livekit/` directory, `LiveKitEventHandler::new(inner, source, sample_rate, channels)`.

**Migration:**
```rust
// Before (Mike)
let handler = LiveKitEventHandler::new(source, Arc::new(inner));

// After
let handler = LiveKitEventHandler::new(inner, source, 24000, 1);
```

### 9. Error Types

We added transport-specific variants. These are additive — existing match arms still work:

```rust
RealtimeError::OpusCodecError(String)  // Opus encode/decode failures
RealtimeError::WebRTCError(String)     // WebRTC connection/signaling failures
RealtimeError::LiveKitError(String)    // LiveKit bridge failures
```

### 10. Facade Crate (`adk-rust`)

**Mike:** Only `realtime` and `livekit` forwarded.

**Ours:** `realtime`, `vertex-live`, `livekit`, `openai-webrtc` all forwarded, plus prelude re-exports for `RealtimeAgent`, `RealtimeAgentBuilder`, `RealtimeConfig`, `RealtimeModel`, `RealtimeRunner`, `RealtimeSession`, `RealtimeSessionExt`, `BoxedSession`, `BoxedModel`.

### 11. Convenience APIs

These already exist in our implementation (no migration needed, just use them):

| API | Description |
|-----|-------------|
| `BoxedSession` | `type BoxedSession = Box<dyn RealtimeSession>` |
| `BoxedModel` | `type BoxedModel = Arc<dyn RealtimeModel>` |
| `RealtimeSessionExt` | Blanket trait with `send_text_and_wait()`, `send_audio_and_wait()`, `collect_audio()` |
| `GeminiLiveBackend::studio(key)` | Convenience constructor for Studio backend |
| `GeminiLiveBackend::vertex_adc(project, region)` | Convenience constructor for Vertex ADC |

---

## What Mike Gets By Migrating

- 5 property test suites (100 iterations each): Vertex URL, LiveKit delegation, Opus round-trip, SDP offer structure, error context
- Integration tests (`#[ignore]`) for all three transports with timeout guards
- Examples: `vertex_live_voice`, `livekit_bridge`, `openai_webrtc` with READMEs
- Ephemeral token SDP signaling (OpenAI recommended)
- cmake-free default build (`full` doesn't require cmake)
- Comprehensive README with architecture diagrams
- ROADMAP with known issues and future work
- `RealtimeSessionExt` convenience methods
- `vertex_adc()` ergonomic constructor
- Data channel pre-open buffering
- `AudioFormat` metadata on client audio events

---

## Step-by-Step Migration

1. **Switch branch:**
   ```bash
   git checkout feat/realtime-audio-transport
   ```

2. **Update feature flags** in scripts/CI:
   - `webrtc` → `openai-webrtc`
   - `vertex` → `vertex-live`
   - `full` no longer includes WebRTC; use `full-webrtc` for everything

3. **Update imports:**
   ```rust
   // Before
   use adk_realtime::openai::OpenAiWebRtcModel;
   use adk_gemini::GeminiLiveBackend;

   // After
   use adk_realtime::openai::{OpenAIRealtimeModel, OpenAITransport};
   use adk_realtime::gemini::GeminiLiveBackend;
   ```

4. **Update WebRTC model creation:**
   ```rust
   // Before
   let model = OpenAiWebRtcModel::new("gpt-4o-realtime-preview", api_key);

   // After
   let model = OpenAIRealtimeModel::new(api_key, "gpt-4o-realtime-preview")
       .with_transport(OpenAITransport::WebRTC);
   ```

5. **Update Vertex AI connection:**
   ```rust
   // Before
   let backend = GeminiLiveBackend::VertexADC { project, location };

   // After
   let backend = GeminiLiveBackend::vertex_adc(project, location)?;
   ```

6. **Update LiveKit handler:**
   ```rust
   // Before
   let handler = LiveKitEventHandler::new(source, Arc::new(inner));

   // After
   let handler = LiveKitEventHandler::new(inner, source, 24000, 1);
   ```

7. **Update audio event handling** (if using `Bytes`):
   ```rust
   // Before — delta is Bytes
   ServerEvent::AudioDelta { delta, .. } => { delta.as_ref() }

   // After — delta is Vec<u8>
   ServerEvent::AudioDelta { delta, .. } => { delta.as_slice() }
   ```

8. **Build with cmake env var** (WebRTC only):
   ```bash
   export CMAKE_POLICY_VERSION_MINIMUM=3.5
   cargo check -p adk-realtime --features openai-webrtc
   ```

9. **Verify:**
   ```bash
   cargo test -p adk-realtime --test error_context_tests
   cargo test -p adk-realtime --test events_tests
   cargo test -p adk-realtime --features vertex-live --test vertex_url_property_tests
   cargo test -p adk-realtime --features livekit --test livekit_delegation_tests
   CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo test -p adk-realtime --features openai-webrtc --test opus_roundtrip_tests
   CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo test -p adk-realtime --features openai-webrtc --test sdp_offer_tests
   ```

---

## Files Not Relevant to Migration

| File | Notes |
|------|-------|
| `.cargo/config.toml` | Mike's Linux dev optimization (wild linker, sccache). Keep if desired. |
| `devenv.nix` | Mike's Nix environment. Keep if desired. |
| `adk-studio/src/server/cors.rs` | We have CORS handling in `main.rs` already. |
