//! Gemini Live session implementation.
//!
//! Manages a WebSocket connection to Google's Gemini Live API with support
//! for both AI Studio (API key) and Vertex AI (OAuth/ADC) backends.

use crate::audio::{AudioChunk, AudioFormat};
use crate::config::{FunctionResponseScheduling, RealtimeConfig, ToolDefinition, VadMode};
use crate::error::{RealtimeError, Result};
use crate::events::{ClientEvent, InteractionStatus, ServerEvent, ToolResponse};
use crate::recovery::{
    RealtimeRecovery, RecoveredSession, RecoveryCause, RecoveryContext, RecoveryContinuity,
    RecoveryDisposition,
};
use crate::session::{ContextMutationOutcome, RealtimeLifecycle, RealtimeSession};
use adk_gemini::schema_adapter::GeminiSchemaDialect;
use async_trait::async_trait;
use base64::Engine;
use bytes::{BufMut, BytesMut};
use futures::stream::Stream;
use futures::{SinkExt, StreamExt};
use parking_lot::Mutex as ParkingMutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{connect_async, tungstenite::Message};

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type WsSource = futures::stream::SplitStream<WsStream>;
const WRITER_CHANNEL_CAPACITY: usize = 64;
const AUDIO_FLUSH_TARGET_MS: usize = 40;

/// Bare Gemini Live model ids, without the `models/` resource prefix.
/// `capabilities_for` strips the prefix before matching, so the gate sees
/// these identifiers whether the caller passes a bare id or a resource name.
/// The per-model support matrix comes from the Live capabilities docs: the
/// 3.8 family (Live, Live Extended Thinking) honors both conversational
/// knobs — `proactivity.proactiveAudio` at the setup root and
/// `generationConfig.enableAffectiveDialog` — while 3.1 Flash Live honors
/// neither. Unknown ids fail closed to the safe baseline below.
const GEMINI_38_LIVE: &str = "gemini-3.8-live";
const GEMINI_38_LIVE_EXTENDED_THINKING: &str = "gemini-3.8-live-extended-thinking";
const GEMINI_31_FLASH_LIVE_PREVIEW: &str = "gemini-3.1-flash-live-preview";

/// Feature flags and wire capabilities for Gemini Live models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GeminiLiveCapabilities {
    pub configurable_thinking: bool,
    pub affective_dialog: bool,
    pub proactive_audio: bool,
    pub cached_content: bool,
    pub audio_only_response: bool,
    pub async_functions: bool,
}

/// Resolve wire capabilities strictly by model identity.
pub(crate) fn capabilities_for(model: &str) -> GeminiLiveCapabilities {
    let normalized = normalize_model_id(model);
    let identifier = normalized.strip_prefix("models/").unwrap_or(&normalized);

    match identifier {
        // Standard Gemini 3.8 Live: interleaved reasoning, audio-only, async functions by default
        GEMINI_38_LIVE => GeminiLiveCapabilities {
            configurable_thinking: false, // thinking_config rejected
            affective_dialog: true,       // generationConfig.enableAffectiveDialog honored
            proactive_audio: true,        // proactivity.proactiveAudio honored
            cached_content: false,        // Live stream caching unsupported
            audio_only_response: true,    // Output modality must be AUDIO
            async_functions: true,        // NON_BLOCKING wire behavior supported
        },

        // Gemini 3.8 Live Extended Thinking: supports thinking_config, requires async tools
        GEMINI_38_LIVE_EXTENDED_THINKING => GeminiLiveCapabilities {
            configurable_thinking: true, // thinking_config supported
            affective_dialog: true,      // generationConfig.enableAffectiveDialog honored
            proactive_audio: true,       // proactivity.proactiveAudio honored
            cached_content: false,
            audio_only_response: true,
            async_functions: true,
        },

        // Legacy 3.1 Flash Live Preview
        GEMINI_31_FLASH_LIVE_PREVIEW => GeminiLiveCapabilities {
            configurable_thinking: true, // Accepts thinking_level in setup
            affective_dialog: false,     // Live docs exclude 3.1 Flash Live
            proactive_audio: false,      // Live docs exclude 3.1 Flash Live
            cached_content: false,       // Live stream caching unsupported
            audio_only_response: false,  // Allowed ["AUDIO", "TEXT"]
            async_functions: false,      // Defaulted to blocking tool execution
        },

        // Safe baseline for unknown / experimental live endpoints
        _ => GeminiLiveCapabilities {
            configurable_thinking: false,
            affective_dialog: false,
            proactive_audio: false,
            cached_content: false,
            audio_only_response: true,
            async_functions: false,
        },
    }
}

/// Backend configuration for Gemini Live connections.
///
/// Determines how to authenticate and which endpoint to connect to.
#[derive(Clone)]
pub enum GeminiLiveBackend {
    /// AI Studio with API key authentication.
    Studio { api_key: String, endpoint_url: Option<String> },

    /// Vertex AI with OAuth2/ADC authentication.
    #[cfg(feature = "vertex-live")]
    Vertex {
        /// Google Cloud credentials for OAuth2 token generation.
        credentials: google_cloud_auth::credentials::Credentials,
        /// Google Cloud region (e.g., "us-central1").
        region: String,
        /// Google Cloud project ID.
        project_id: String,
        /// Custom WebSocket URL endpoint override.
        endpoint_url: Option<String>,
    },
}

/// Hand-written so the credential never reaches a log line.
///
/// A derived `Debug` prints `api_key` verbatim, and this type is held by
/// `GeminiRealtimeSession`, so a single `tracing::debug!(?backend, ..)` -- or a
/// `#[derive(Debug)]` added to any struct that owns one -- is enough to put a
/// live Google API key into a log aggregator. The region, project, and the
/// addressable part of the endpoint stay visible, because those are what anyone
/// reading the log actually needs.
impl std::fmt::Debug for GeminiLiveBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GeminiLiveBackend::Studio { api_key, endpoint_url } => f
                .debug_struct("Studio")
                .field("api_key", &Redacted(api_key.len()))
                .field("endpoint_url", &endpoint_url.as_deref().map(EndpointUrl))
                .finish(),
            #[cfg(feature = "vertex-live")]
            GeminiLiveBackend::Vertex { credentials: _, region, project_id, endpoint_url } => f
                .debug_struct("Vertex")
                .field("credentials", &"<redacted>")
                .field("region", region)
                .field("project_id", project_id)
                .field("endpoint_url", &endpoint_url.as_deref().map(EndpointUrl))
                .finish(),
        }
    }
}

/// An endpoint with its query string withheld.
///
/// `with_endpoint_url` documents that ADK attaches no Google credential to a
/// custom host, so whatever authenticates that host is the caller's own -- and
/// the conventional place to put it is the query string. Printing the endpoint
/// verbatim to make the field useful would hand that back out through the same
/// `Debug` the `api_key` redaction exists to close.
///
/// Scheme, host, and path survive, which is what identifies which endpoint was
/// dialled; everything from the first `?` is withheld. The presence of a query
/// is still visible, because "the proxy got no token" and "the proxy got the
/// wrong token" are different bugs.
struct EndpointUrl<'a>(&'a str);

impl std::fmt::Debug for EndpointUrl<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0.split_once('?') {
            Some((addressable, _)) => write!(f, "{addressable}?<redacted>"),
            None => write!(f, "{}", self.0),
        }
    }
}

/// Renders as `<redacted N bytes>`.
///
/// The length is kept because "the key is empty" and "the key is present but
/// wrong" are different bugs, and a bare `<redacted>` cannot tell them apart.
struct Redacted(usize);

impl std::fmt::Debug for Redacted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<redacted {} bytes>", self.0)
    }
}

impl GeminiLiveBackend {
    /// Create a Studio backend with API key authentication.
    pub fn studio(api_key: impl Into<String>) -> Self {
        Self::Studio { api_key: api_key.into(), endpoint_url: None }
    }

    /// Override the WebSocket connection URL for mock servers or custom proxies.
    ///
    /// # Security Policy
    ///
    /// When connecting to a custom endpoint URL specified via `with_endpoint_url()`,
    /// ADK **will not** attach or forward Google credentials (such as Google API keys
    /// or Google ADC bearer tokens) to the custom host. This prevents credential leakage
    /// when proxying through untrusted custom endpoints or local mock servers.
    pub fn with_endpoint_url(mut self, endpoint_url: impl Into<String>) -> Self {
        let ep = Some(endpoint_url.into());
        match &mut self {
            GeminiLiveBackend::Studio { endpoint_url: e, .. } => *e = ep,
            #[cfg(feature = "vertex-live")]
            GeminiLiveBackend::Vertex { endpoint_url: e, .. } => *e = ep,
        }
        self
    }

    /// Create a Vertex AI backend using Application Default Credentials (ADC).
    ///
    /// This is the most ergonomic way to connect to Vertex AI Live. It
    /// automatically discovers credentials from the environment using
    /// `google-cloud-auth`'s default credential chain (environment variables,
    /// `gcloud auth application-default login`, service account files, etc.).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let backend = GeminiLiveBackend::vertex_adc("my-project", "us-central1")?;
    /// let model = GeminiRealtimeModel::new(backend, "models/gemini-3.1-flash-live-preview");
    /// ```
    #[cfg(feature = "vertex-live")]
    pub fn vertex_adc(project_id: impl Into<String>, region: impl Into<String>) -> Result<Self> {
        let credentials =
            google_cloud_auth::credentials::Builder::default().build().map_err(|e| {
                RealtimeError::AuthError(format!(
                    "Failed to discover Application Default Credentials: {e}"
                ))
            })?;
        Ok(Self::Vertex {
            credentials,
            region: region.into(),
            project_id: project_id.into(),
            endpoint_url: None,
        })
    }
}

// ── Wire format types ───────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GeminiClientMessage<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    setup: Option<GeminiSetup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    realtime_input: Option<GeminiRealtimeInput<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_response: Option<GeminiToolResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_content: Option<GeminiClientContent>,
}

/// Configuration for Gemini 3.8 Live session resumption.
///
/// See the official documentation for details:
/// https://ai.google.dev/gemini-api/docs/live-api/session-management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResumptionConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiSetup {
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cached_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_resumption: Option<SessionResumptionConfig>,
    /// Enable transcription of the user's input audio (empty object = on).
    #[serde(skip_serializing_if = "Option::is_none")]
    input_audio_transcription: Option<Value>,
    /// Enable transcription of the model's spoken output (empty object = on).
    #[serde(skip_serializing_if = "Option::is_none")]
    output_audio_transcription: Option<Value>,
    /// Voice-activity detection and interruption policy. Absent until now, so
    /// every `VadConfig` the caller set was accepted and discarded.
    #[serde(skip_serializing_if = "Option::is_none")]
    realtime_input_config: Option<GeminiRealtimeInputConfig>,
    /// Only sent when the caller asked for it
    /// ([`RealtimeConfig::with_context_window_compression`]). Google documents that without
    /// it a session ends after about 2 minutes with video and about 15 audio-only.
    #[serde(skip_serializing_if = "Option::is_none")]
    context_window_compression: Option<GeminiContextWindowCompression>,
    /// Proactive behavior policy (`{ "proactiveAudio": true }`): lets the
    /// model ignore irrelevant audio instead of responding to it. Omitted
    /// unless the model honors it — see `capabilities_for`.
    #[serde(skip_serializing_if = "Option::is_none")]
    proactivity: Option<Value>,
}

/// `setup.realtimeInputConfig`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiRealtimeInputConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    automatic_activity_detection: Option<GeminiAutomaticActivityDetection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    activity_handling: Option<GeminiActivityHandling>,
}

/// `setup.realtimeInputConfig.automaticActivityDetection`.
///
/// `disabled` is always written in an emitted object: it decides who owns
/// turn detection, and a partial object without it reached production once
/// and correlated with a silent session (see `gemini_realtime_input_config`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiAutomaticActivityDetection {
    disabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    silence_duration_ms: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prefix_padding_ms: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_of_speech_sensitivity: Option<GeminiStartOfSpeechSensitivity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    end_of_speech_sensitivity: Option<GeminiEndOfSpeechSensitivity>,
}

/// Wire values for `startOfSpeechSensitivity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
enum GeminiStartOfSpeechSensitivity {
    #[serde(rename = "START_SENSITIVITY_HIGH")]
    High,
    #[serde(rename = "START_SENSITIVITY_LOW")]
    Low,
}

/// Wire values for `endOfSpeechSensitivity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
enum GeminiEndOfSpeechSensitivity {
    #[serde(rename = "END_SENSITIVITY_HIGH")]
    High,
    #[serde(rename = "END_SENSITIVITY_LOW")]
    Low,
}

/// Wire values for `activityHandling`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
enum GeminiActivityHandling {
    #[serde(rename = "START_OF_ACTIVITY_INTERRUPTS")]
    StartOfActivityInterrupts,
    #[serde(rename = "NO_INTERRUPTION")]
    NoInterruption,
}

/// `setup.contextWindowCompression`.
///
/// `slidingWindow` is the only mechanism Google defines; an empty one keeps the server's
/// defaults (compress at 80% of the context window, cut back to half of that).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiContextWindowCompression {
    sliding_window: GeminiSlidingWindow,
    #[serde(skip_serializing_if = "Option::is_none")]
    trigger_tokens: Option<u64>,
}

/// `setup.contextWindowCompression.slidingWindow`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiSlidingWindow {
    #[serde(skip_serializing_if = "Option::is_none")]
    target_tokens: Option<u64>,
}

impl GeminiContextWindowCompression {
    /// Reads `extra.context_window_compression`: `true` for the server defaults, or an object
    /// with optional `trigger_tokens` / `target_tokens`. Anything else leaves it off.
    fn from_extra(extra: Option<&Value>) -> Option<Self> {
        match extra?.get("context_window_compression")? {
            Value::Bool(true) => Some(Self::default()),
            Value::Object(opts) => Some(Self {
                sliding_window: GeminiSlidingWindow {
                    target_tokens: opts.get("target_tokens").and_then(Value::as_u64),
                },
                trigger_tokens: opts.get("trigger_tokens").and_then(Value::as_u64),
            }),
            _ => None,
        }
    }
}

/// `activityStart` / `activityEnd`.
///
/// Both are documented as having no fields, so they serialize as `{}`. The
/// marker exists only so the `Option` in [`GeminiRealtimeInput`] can be `Some`
/// without inventing a field Google does not define.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
struct GeminiActivityMarker {}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GeminiContent {
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GeminiPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inline_data: Option<GeminiInlineData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiInlineData {
    mime_type: String,
    data: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiRealtimeInput<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    audio: Option<GeminiMediaChunk<'a>>,
    /// Camera / screen frames. Not the deprecated `mediaChunks`: `gemini-3.8-live` drops
    /// images sent there without an error, and the model reports seeing nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    video: Option<GeminiMediaChunk<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    /// The user has started speaking. Only legal while automatic activity
    /// detection is disabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    activity_start: Option<GeminiActivityMarker>,
    /// The user has stopped speaking. Only legal while automatic activity
    /// detection is disabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    activity_end: Option<GeminiActivityMarker>,
    /// The audio stream has paused: the server flushes the audio it cached, so its
    /// activity detection can close the turn. Sent while automatic detection is on.
    #[serde(skip_serializing_if = "Option::is_none")]
    audio_stream_end: Option<bool>,
}

/// Borrows both fields: this is the hot path.
///
/// Audio ingress sends one of these every 20 ms (~50/sec), so owning the
/// fields meant a fresh `String` for a *constant* mime type plus a full copy
/// of the ~856-byte base64 payload on every frame — roughly 30k allocations
/// and 12 MB copied over a five-minute call, all discarded immediately after
/// `serde_json` wrote them into the outbound buffer. `send_raw` only needs
/// `&impl Serialize`, so borrowing costs nothing (repo Law 3: zero-copy hot
/// paths).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiMediaChunk<'a> {
    mime_type: &'a str,
    data: &'a str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiToolResponse {
    function_responses: Vec<GeminiFunctionResponse>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiFunctionResponse {
    id: String,
    name: String,
    response: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    scheduling: Option<FunctionResponseScheduling>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiClientContent {
    turns: Vec<GeminiTurn>,
    turn_complete: bool,
}

// ── Server wire types ───────────────────────────────────────────────────
//
// The rest of this translator reads server frames by poking at a
// `serde_json::Value` with `.get("someField")`, which puts the wire contract
// in string literals scattered through one long function. These are the
// beginning of a typed inbound layer; new server messages should land here
// rather than adding another `.get`.
//
// Deliberately permissive — no `deny_unknown_fields`. Gemini Live is a
// Preview surface and adds fields; an adapter that refuses to parse a frame
// carrying something new would take a live call down over a field it did not
// need. Strictness belongs in the normalization step, not here.

/// `BidiGenerateContentToolCallCancellation`: function calls the model had
/// already issued and has now withdrawn, normally because the caller
/// interrupted the turn that produced them.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiToolCallCancellation {
    /// Ids matching `id`s previously delivered in a `toolCall` frame.
    ///
    /// Defaulted rather than required: a frame with the key present but no
    /// ids withdraws nothing, which is a no-op rather than a protocol error.
    #[serde(default)]
    ids: Vec<String>,
}

/// `UsageMetadata` on a Live server message: token counts for completed
/// response(s), plus per-modality splits where Google sends them.
///
/// Outside the `messageType` union, so it rides alongside whatever else the
/// frame carries — in practice `serverContent` at turn end, or alone —
/// rather than as one more exclusive arm. Counts only, never transcript or
/// audio.
///
/// Field names follow the Live reference, which says `responseTokenCount` /
/// `responseTokensDetails` where the REST surface says `candidates*`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsageMetadata {
    prompt_token_count: Option<u32>,
    cached_content_token_count: Option<u32>,
    response_token_count: Option<u32>,
    tool_use_prompt_token_count: Option<u32>,
    thoughts_token_count: Option<u32>,
    total_token_count: Option<u32>,
    prompt_tokens_details: Option<Vec<GeminiModalityTokenCount>>,
    cache_tokens_details: Option<Vec<GeminiModalityTokenCount>>,
    response_tokens_details: Option<Vec<GeminiModalityTokenCount>>,
    tool_use_prompt_tokens_details: Option<Vec<GeminiModalityTokenCount>>,
}

/// One Live `ModalityTokenCount`: how many of the tokens were of one modality.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiModalityTokenCount {
    modality: Option<String>,
    token_count: Option<u32>,
}

impl From<GeminiModalityTokenCount> for crate::events::ModalityTokenCount {
    fn from(value: GeminiModalityTokenCount) -> Self {
        Self { modality: value.modality, token_count: value.token_count }
    }
}

impl From<GeminiUsageMetadata> for crate::events::Usage {
    fn from(value: GeminiUsageMetadata) -> Self {
        let details = |entries: Option<Vec<GeminiModalityTokenCount>>| {
            entries.map(|v| v.into_iter().map(crate::events::ModalityTokenCount::from).collect())
        };
        Self {
            prompt_token_count: value.prompt_token_count,
            response_token_count: value.response_token_count,
            total_token_count: value.total_token_count,
            cached_content_token_count: value.cached_content_token_count,
            thoughts_token_count: value.thoughts_token_count,
            tool_use_prompt_token_count: value.tool_use_prompt_token_count,
            prompt_tokens_details: details(value.prompt_tokens_details),
            response_tokens_details: details(value.response_tokens_details),
            cache_tokens_details: details(value.cache_tokens_details),
            tool_use_prompt_tokens_details: details(value.tool_use_prompt_tokens_details),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiTurn {
    role: String,
    parts: Vec<GeminiPart>,
}

// ── Activity detection ──────────────────────────────────────────────────

/// Which side of the connection decides when the user is speaking.
///
/// Gemini Live treats this as a single choice, not two independent switches:
/// `activityStart` and `activityEnd` "can only be sent if automatic (i.e.
/// server-side) activity detection is disabled". Enabling client signalling is
/// therefore not an addition to server VAD, it is a transfer of ownership, and
/// this enum is the record of who holds it for the life of a session.
///
/// The mode is fixed at connect time because it is carried in the `setup`
/// frame, which Gemini Live accepts exactly once per connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActivityDetection {
    /// Gemini's own VAD segments turns. This is the API default and what every
    /// session gets unless it explicitly asks otherwise.
    ///
    /// Client activity frames are rejected by the server in this mode, so
    /// [`GeminiRealtimeSession::activity_signaller`] yields nothing here.
    #[default]
    Automatic,
    /// `setup.realtimeInputConfig.automaticActivityDetection.disabled = true`.
    ///
    /// The server performs no turn detection at all; the client must send
    /// `activityStart` and `activityEnd` itself. Selecting this without
    /// actually sending those frames leaves the session with no turn detection
    /// on either side.
    Manual,
}

impl ActivityDetection {
    /// Resolve the mode a config asks for.
    ///
    /// [`VadMode::None`] is the only way to reach [`Manual`](Self::Manual):
    /// it is already the crate's provider-agnostic word for "no automatic turn
    /// detection", and honouring it here is what stops it being a request that
    /// silently does nothing on Gemini.
    fn from_config(config: &RealtimeConfig) -> Self {
        match config.turn_detection.as_ref().map(|vad| vad.mode) {
            Some(VadMode::None) => Self::Manual,
            // Absent config, `ServerVad` and `SemanticVad` all leave detection
            // with the server. `SemanticVad` has no Gemini equivalent, and
            // downgrading it to the server default is strictly closer to its
            // intent than disabling detection entirely.
            _ => Self::Automatic,
        }
    }
}

/// What an activity signal actually did.
///
/// Returned rather than swallowed because "the frame went out" and "the
/// session was already in that state" are different facts, and a caller that
/// records one as the other is claiming to have told the server something it
/// never said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum ActivitySignalOutcome {
    /// The frame was written to the connection.
    Sent,
    /// Nothing was sent: the session was already in the requested state, so
    /// this was a duplicate report of one activity transition.
    Redundant,
}

/// Permission to send `activityStart` / `activityEnd` on a session that
/// negotiated [`ActivityDetection::Manual`].
///
/// This type exists so the protocol's exclusivity rule is enforced by
/// construction rather than by remembering to check. It is reachable only
/// through [`GeminiRealtimeSession::activity_signaller`], which returns `None`
/// for an automatic-detection session, so there is no way to hold one for a
/// session whose server would reject the frames.
///
/// # One transition, one frame
///
/// `start` and `end` are edge-triggered against a per-session activity state,
/// so calling `start` twice without an intervening `end` sends one frame and
/// reports [`ActivitySignalOutcome::Redundant`] for the second.
///
/// This is deliberate. A caller that fuses several detectors, or that reacts
/// both to its own detector and to inbound `serverContent.interrupted`, has
/// more signals than there are transitions. RFC 6787 §6.2.4 exists because a
/// duplicate cancel for a *single* barge-in cancels the *next* prompt, and the
/// cheapest place to stop that is where the frames are written.
///
/// The de-duplication is per state transition on this session, which is only
/// the outbound half. It cannot tell whether an inbound interruption report was
/// caused by a frame sent here — Gemini's `activityStart` carries no fields and
/// `serverContent.interrupted` carries no correlation id, so nothing in the
/// protocol supports that inference and this type does not fake one.
///
/// # Example
///
/// ```rust,ignore
/// // `None` on a server-VAD session — the frames would be rejected.
/// if let Some(activity) = session.activity_signaller() {
///     activity.start().await?;
///     session.send_audio(&chunk).await?;
///     activity.end().await?;
/// }
/// ```
#[derive(Debug, Clone, Copy)]
pub struct ActivitySignaller<'a> {
    session: &'a GeminiRealtimeSession,
}

impl ActivitySignaller<'_> {
    /// Report that the user has started speaking.
    ///
    /// Google documents `activityStart` as interrupting the model's response
    /// under the default `activityHandling` of `START_OF_ACTIVITY_INTERRUPTS`.
    /// What this method guarantees is narrower and is all it will claim: the
    /// frame was written to the connection. Whether the server then stopped
    /// generating, and whether it reports that back as
    /// `serverContent.interrupted`, is the server's behaviour and is not
    /// observed here.
    ///
    /// Audio that arrives before this frame is not attributed to the turn, so
    /// send a little leading context after it rather than clipping to the
    /// exact speech onset.
    pub async fn start(&self) -> Result<ActivitySignalOutcome> {
        self.session.send_activity(ActivitySignal::Start).await
    }

    /// Report that the user has stopped speaking, ending the turn.
    pub async fn end(&self) -> Result<ActivitySignalOutcome> {
        self.session.send_activity(ActivitySignal::End).await
    }
}

/// Which of the two activity frames to emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActivitySignal {
    Start,
    End,
}

impl ActivitySignal {
    fn into_realtime_input(self) -> GeminiRealtimeInput<'static> {
        let marker = Some(GeminiActivityMarker {});
        match self {
            Self::Start => GeminiRealtimeInput { activity_start: marker, ..Default::default() },
            Self::End => GeminiRealtimeInput { activity_end: marker, ..Default::default() },
        }
    }

    /// The activity state this signal moves the session into.
    fn target_state(self) -> bool {
        matches!(self, Self::Start)
    }
}

// ── Session implementation ──────────────────────────────────────────────

/// Gemini Live session.
///
/// Manages a WebSocket connection to Google's Gemini Live API.
pub struct GeminiRealtimeSession {
    session_id: String,
    connected: Arc<AtomicBool>,
    cancel_token: Arc<Mutex<tokio_util::sync::CancellationToken>>,
    outbound_tx: Arc<Mutex<mpsc::Sender<Message>>>,
    writer_task: Arc<Mutex<Option<JoinHandle<()>>>>,
    receiver: Arc<Mutex<WsSource>>,
    audio_buffer: Arc<ParkingMutex<BytesMut>>,
    event_queue: Arc<ParkingMutex<std::collections::VecDeque<ServerEvent>>>,
    frame_log: FrameLog,
    /// The close frame the server sent, if it sent one.
    ///
    /// Recorded because the event stream reports a deliberate server-side
    /// close and a dead socket as the same `None`, so a polling caller cannot
    /// tell an aborted session from a broken one without it.
    last_disconnect: Arc<ParkingMutex<Option<crate::session::DisconnectReason>>>,
    backend: GeminiLiveBackend,
    dialect: GeminiSchemaDialect,
    /// The normalized model ID sent in the initial setup frame.
    reconnect_model: String,
    /// The most recent resumable handle received via `sessionResumptionUpdate`,
    /// with the instant it was latched.
    ///
    /// Gemini Live emits a new handle roughly every 60 s when `resumable: true`.
    /// Only resumable handles are stored here; a non-resumable update must not
    /// overwrite the last safe point (the previous resumable handle remains the
    /// best reconnect anchor). A handle older than
    /// [`RESUME_HANDLE_TTL`] is dropped rather than presented on reconnect.
    last_resume_handle: Arc<ParkingMutex<Option<(String, std::time::Instant)>>>,

    /// Association map between function call IDs and tool names for wire-conforming `FunctionResponse` payloads.
    call_names: Arc<ParkingMutex<CallNamesMap>>,
    /// Watch channel broadcasting replacement deadlines announced by Gemini Live `goAway` frames.
    replacement_deadline_tx: tokio::sync::watch::Sender<Option<std::time::Instant>>,
    replacement_deadline_rx: tokio::sync::watch::Receiver<Option<std::time::Instant>>,
    /// Who owns turn detection on this connection.
    ///
    /// Derived from the same config that produced the `setup` frame, so it
    /// cannot disagree with what the server was told.
    activity_detection: ActivityDetection,
    /// Whether an `activityStart` is currently outstanding (manual mode only).
    ///
    /// Makes the outbound signal edge-triggered: several observers of one
    /// barge-in collapse to a single frame. See [`ActivitySignaller`].
    activity_open: Arc<AtomicBool>,
    /// Whether this connection ever received `setupComplete`.
    ///
    /// The discriminator between "the provider dropped a working session" and
    /// "the provider rejected our setup". Both surface as the same end of
    /// stream, but only the first is worth recovering: recovery replays the
    /// current effective configuration, which is precisely the request the
    /// server just refused. See [`RealtimeRecovery::classify`].
    setup_complete_seen: Arc<AtomicBool>,
    /// The latest `usageMetadata` the server sent on a message that carried nothing else,
    /// attached to the next `ResponseDone` so a usage report separate from the turn is not
    /// lost.
    last_usage: Arc<ParkingMutex<Option<Value>>>,
    /// Whether `voiceActivity` reported the user's speech onset and not yet its end, so the
    /// `interrupted` report of the same speech is not a second speech event.
    speech_active: Arc<AtomicBool>,
}

const MAX_CALL_NAMES_CAPACITY: usize = 1000;

/// How long a latched resumption handle may be presented on reconnect.
const RESUME_HANDLE_TTL: std::time::Duration = std::time::Duration::from_secs(10 * 60);

#[derive(Debug, Default)]
pub(crate) struct CallNamesMap {
    order: std::collections::VecDeque<String>,
    map: std::collections::HashMap<String, String>,
}

impl CallNamesMap {
    pub(crate) fn new() -> Self {
        Self { order: std::collections::VecDeque::new(), map: std::collections::HashMap::new() }
    }

    pub(crate) fn insert(&mut self, call_id: String, name: String) {
        if !self.map.contains_key(&call_id) {
            self.order.push_back(call_id.clone());
            if self.order.len() > MAX_CALL_NAMES_CAPACITY
                && let Some(oldest) = self.order.pop_front()
            {
                self.map.remove(&oldest);
            }
        }
        self.map.insert(call_id, name);
    }

    pub(crate) fn remove(&mut self, call_id: &str) -> Option<String> {
        let removed = self.map.remove(call_id);
        if removed.is_some() {
            self.order.retain(|id| id != call_id);
        }
        removed
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }
}

/// Ensure the model ID is prefixed with "models/" as required by Gemini Live setup frames.
pub fn normalize_model_id(model_id: &str) -> String {
    if model_id.starts_with("models/")
        || model_id.starts_with("tunedModels/")
        || model_id.starts_with("projects/")
        || model_id.starts_with("publishers/")
    {
        model_id.to_string()
    } else {
        format!("models/{}", model_id)
    }
}

impl GeminiRealtimeSession {
    fn flush_threshold_bytes(format: &AudioFormat) -> usize {
        let bytes_per_second = format.bytes_per_second() as usize;
        // Compute target bytes for a 40ms chunk and round up so we don't under-buffer.
        // `max(1)` keeps the threshold valid even for pathological/invalid formats.
        bytes_per_second.saturating_mul(AUDIO_FLUSH_TARGET_MS).div_ceil(1000).max(1)
    }

    /// Returns the last resumable handle received via `sessionResumptionUpdate`.
    #[cfg(any(test, feature = "recovery-test-utils"))]
    pub fn last_resume_handle(&self) -> Option<String> {
        self.last_resume_handle.lock().as_ref().map(|(handle, _)| handle.clone())
    }

    #[cfg(not(any(test, feature = "recovery-test-utils")))]
    pub(crate) fn last_resume_handle(&self) -> Option<String> {
        self.last_resume_handle.lock().as_ref().map(|(handle, _)| handle.clone())
    }

    /// The latched handle if it is still inside its TTL; an expired
    /// handle is dropped and reported as absent.
    fn fresh_resume_handle(&self) -> Option<String> {
        let mut latch = self.last_resume_handle.lock();
        match latch.as_ref() {
            Some((handle, latched)) if latched.elapsed() <= RESUME_HANDLE_TTL => {
                Some(handle.clone())
            }
            Some(_) => {
                *latch = None;
                None
            }
            None => None,
        }
    }

    /// Constructs a mock `GeminiRealtimeSession` for testing connection lifecycle & recovery.
    #[cfg(any(test, feature = "recovery-test-utils"))]
    pub fn new_for_test(
        session_id: String,
        reconnect_url: String,
        reconnect_model: String,
        outbound_tx: mpsc::Sender<Message>,
        writer_task: tokio::task::JoinHandle<()>,
        receiver: WsSource,
    ) -> Self {
        use std::sync::atomic::AtomicBool;
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let backend = GeminiLiveBackend::studio("mock-api-key").with_endpoint_url(reconnect_url);
        let dialect = GeminiSchemaDialect::OpenApiSubset;
        let (replacement_deadline_tx, replacement_deadline_rx) = tokio::sync::watch::channel(None);
        Self {
            session_id: session_id.clone(),
            connected: Arc::new(AtomicBool::new(true)),
            cancel_token: Arc::new(Mutex::new(cancel_token)),
            outbound_tx: Arc::new(Mutex::new(outbound_tx)),
            writer_task: Arc::new(Mutex::new(Some(writer_task))),
            receiver: Arc::new(Mutex::new(receiver)),
            audio_buffer: Arc::new(ParkingMutex::new(BytesMut::new())),
            event_queue: Arc::new(ParkingMutex::new(std::collections::VecDeque::new())),
            frame_log: FrameLog::new(session_id, "studio", reconnect_model.clone()),
            last_disconnect: Arc::new(ParkingMutex::new(None)),
            backend,
            dialect,
            reconnect_model,
            last_resume_handle: Arc::new(ParkingMutex::new(None)),
            call_names: Arc::new(ParkingMutex::new(CallNamesMap::new())),
            replacement_deadline_tx,
            replacement_deadline_rx,
            activity_detection: ActivityDetection::Automatic,
            activity_open: Arc::new(AtomicBool::new(false)),
            setup_complete_seen: Arc::new(AtomicBool::new(false)),
            last_usage: Arc::new(ParkingMutex::new(None)),
            speech_active: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The dialect a backend uses unless the caller names another.
    ///
    /// Studio's default stays the OpenAPI subset even though
    /// [`GeminiSchemaDialect::JsonSchema`] preserves more, because
    /// `parametersJsonSchema` is absent from Google's reference docs and is
    /// verified only by probe, on one model and one endpoint version. A shared
    /// crate does not change what every consumer puts on the wire on the
    /// strength of that; opting in is one call.
    pub fn default_schema_dialect(backend: &GeminiLiveBackend) -> GeminiSchemaDialect {
        match backend {
            GeminiLiveBackend::Studio { .. } => GeminiSchemaDialect::OpenApiSubset,
            #[cfg(feature = "vertex-live")]
            GeminiLiveBackend::Vertex { .. } => GeminiSchemaDialect::VertexOpenApiSubset,
        }
    }

    /// Connect to Gemini Live API using the specified backend.
    ///
    /// On AI Studio a bare model id such as `gemini-3.1-flash-live-preview` is sent as
    /// `models/gemini-3.1-flash-live-preview`, the resource name the setup requires.
    pub async fn connect(
        backend: GeminiLiveBackend,
        model: &str,
        config: RealtimeConfig,
    ) -> Result<Self> {
        let dialect = Self::default_schema_dialect(&backend);
        Self::connect_with_dialect(backend, model, config, dialect).await
    }

    /// Connect, writing tool schemas in an explicitly chosen dialect.
    ///
    /// The dialect decides both how the schema is reduced and which
    /// function-declaration field carries it, so the two can never disagree.
    /// See [`GeminiSchemaDialect`] for what each one preserves and for the
    /// endpoint evidence behind [`GeminiSchemaDialect::JsonSchema`].
    pub async fn connect_with_dialect(
        backend: GeminiLiveBackend,
        model: &str,
        config: RealtimeConfig,
        dialect: GeminiSchemaDialect,
    ) -> Result<Self> {
        Self::connect_with_dialect_and_resume(backend, model, config, dialect, None).await
    }

    /// Private internal connector that passes a provider-private `private_resume_handle`.
    async fn connect_with_dialect_and_resume(
        backend: GeminiLiveBackend,
        model: &str,
        config: RealtimeConfig,
        dialect: GeminiSchemaDialect,
        private_resume_handle: Option<String>,
    ) -> Result<Self> {
        adk_core::ensure_crypto_provider();
        let normalized_model = normalize_model_id(model);
        let schema_cache = Arc::new(adk_core::SchemaCache::new());
        let adapter: Arc<dyn adk_core::SchemaAdapter> =
            Arc::new(adk_gemini::schema_adapter::GeminiSchemaAdapter::with_dialect(dialect));

        let caps = capabilities_for(&normalized_model);

        // 1. Compile tools BEFORE establishing the WebSocket connection.
        // A tool that fails to compile at all aborts the connect; note the
        // chosen dialect still projects schemas (the OpenApiSubset drops
        // `additionalProperties` and composition) — that loss is deliberate
        // per-dialect, not a compile failure.
        let tools = convert_tools(config.tools.clone(), &schema_cache, adapter.as_ref(), caps)?;

        let ws_stream = match &backend {
            GeminiLiveBackend::Studio { api_key, endpoint_url } => {
                let url = match endpoint_url {
                    Some(u) => u.clone(), // Custom endpoint receives NO Google API key
                    None => format!("{}?key={}", super::GEMINI_LIVE_URL, api_key),
                };
                let request = url.into_client_request().map_err(|e| {
                    RealtimeError::connection(format!("Failed to create request: {}", e))
                })?;
                let (ws, response) = connect_async(request).await.map_err(|e| {
                    RealtimeError::connection(format!("WebSocket connect error: {}", e))
                })?;
                tracing::info!(status = ?response.status(), "Gemini WebSocket handshake successful");
                ws
            }
            #[cfg(feature = "vertex-live")]
            GeminiLiveBackend::Vertex { credentials, region, project_id, endpoint_url } => {
                let url = match endpoint_url {
                    Some(u) => u.clone(),
                    None => build_vertex_live_url(region, project_id)?,
                };

                let mut request = url.into_client_request().map_err(|e| {
                    RealtimeError::connection(format!("Failed to create request: {e}"))
                })?;

                // Attach Google ADC Authorization header ONLY to standard default Google Vertex endpoint
                if endpoint_url.is_none() {
                    let header_map = match credentials.headers(Default::default()).await.map_err(
                        |e| {
                            RealtimeError::AuthError(format!(
                                "Failed to obtain OAuth2 token from ADC credentials: {e}"
                            ))
                        },
                    )? {
                        google_cloud_auth::credentials::CacheableResource::New { data, .. } => data,
                        google_cloud_auth::credentials::CacheableResource::NotModified => {
                            return Err(RealtimeError::AuthError(
                                "ADC credentials returned NotModified with no cached token available"
                                    .to_string(),
                            ));
                        }
                    };

                    let auth_value = header_map
                        .get("authorization")
                        .ok_or_else(|| {
                            RealtimeError::AuthError(
                                "ADC credentials did not produce an Authorization header"
                                    .to_string(),
                            )
                        })?
                        .to_str()
                        .map_err(|e| {
                            RealtimeError::AuthError(format!(
                                "Authorization header contains non-ASCII characters: {e}"
                            ))
                        })?
                        .to_string();

                    request.headers_mut().insert(
                        "Authorization",
                        auth_value.parse().map_err(
                            |e: tokio_tungstenite::tungstenite::http::header::InvalidHeaderValue| {
                                RealtimeError::AuthError(format!(
                                    "Failed to parse Authorization header value: {e}"
                                ))
                            },
                        )?,
                    );
                }

                let (ws, _) = connect_async(request).await.map_err(|e| {
                    RealtimeError::connection(format!(
                        "Vertex AI Live WebSocket connect error: {e}"
                    ))
                })?;
                ws
            }
        };

        let (mut sink, source) = ws_stream.split();
        let connected = Arc::new(AtomicBool::new(true));
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let writer_cancel = cancel_token.clone();
        let (outbound_tx, mut outbound_rx) = mpsc::channel(WRITER_CHANNEL_CAPACITY);
        let writer_connected = Arc::clone(&connected);
        let writer_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = writer_cancel.cancelled() => {
                        tracing::info!("gemini websocket writer task cancelled via token");
                        break;
                    }
                    msg = outbound_rx.recv() => {
                        match msg {
                            Some(message) => {
                                let should_close = matches!(message, Message::Close(_));
                                if let Err(error) = sink.send(message).await {
                                    writer_connected.store(false, Ordering::SeqCst);
                                    tracing::warn!(error = %error, "gemini websocket writer send failed");
                                    break;
                                }
                                if should_close {
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                }
            }
            // Mark disconnected on *any* writer exit path (error, close, channel shutdown).
            writer_connected.store(false, Ordering::SeqCst);
        });

        let session_id = uuid::Uuid::new_v4().to_string();
        let frame_log = FrameLog::new(
            session_id.clone(),
            match &backend {
                GeminiLiveBackend::Studio { .. } => "studio",
                #[cfg(feature = "vertex-live")]
                GeminiLiveBackend::Vertex { .. } => "vertex",
            },
            normalized_model.clone(),
        );

        let (replacement_deadline_tx, replacement_deadline_rx) = tokio::sync::watch::channel(None);
        let activity_detection = ActivityDetection::from_config(&config);

        let session = Self {
            session_id,
            connected,
            cancel_token: Arc::new(Mutex::new(cancel_token)),
            outbound_tx: Arc::new(Mutex::new(outbound_tx)),
            writer_task: Arc::new(Mutex::new(Some(writer_task))),
            receiver: Arc::new(Mutex::new(source)),
            audio_buffer: Arc::new(ParkingMutex::new(BytesMut::new())),
            last_disconnect: Arc::new(ParkingMutex::new(None)),
            event_queue: Arc::new(ParkingMutex::new(std::collections::VecDeque::new())),
            frame_log,
            backend: backend.clone(),
            dialect,
            reconnect_model: normalized_model.clone(),
            last_resume_handle: Arc::new(ParkingMutex::new(
                private_resume_handle.clone().map(|handle| (handle, std::time::Instant::now())),
            )),
            call_names: Arc::new(ParkingMutex::new(CallNamesMap::new())),
            replacement_deadline_tx,
            replacement_deadline_rx,
            activity_detection,
            activity_open: Arc::new(AtomicBool::new(false)),
            setup_complete_seen: Arc::new(AtomicBool::new(false)),
            last_usage: Arc::new(ParkingMutex::new(None)),
            speech_active: Arc::new(AtomicBool::new(false)),
        };

        session
            .send_setup_with_compiled_tools(&normalized_model, config, tools, private_resume_handle)
            .await?;
        Ok(session)
    }

    /// Who owns turn detection on this session.
    pub fn activity_detection(&self) -> ActivityDetection {
        self.activity_detection
    }

    /// Permission to send `activityStart` / `activityEnd`, or `None` if the
    /// server is doing its own activity detection and would reject them.
    ///
    /// See [`ActivitySignaller`].
    pub fn activity_signaller(&self) -> Option<ActivitySignaller<'_>> {
        match self.activity_detection {
            ActivityDetection::Manual => Some(ActivitySignaller { session: self }),
            ActivityDetection::Automatic => None,
        }
    }

    /// Emit one activity frame, if it is a real state transition.
    ///
    /// The mode check is redundant for callers coming through
    /// [`ActivitySignaller`], which cannot exist in automatic mode. It is here
    /// because the alternative to a loud error on the in-crate paths — such as
    /// [`interrupt`](RealtimeSession::interrupt) — is a frame the server
    /// rejects, arriving as an unexplained protocol error later in the stream.
    async fn send_activity(&self, signal: ActivitySignal) -> Result<ActivitySignalOutcome> {
        if self.activity_detection != ActivityDetection::Manual {
            return Err(RealtimeError::config(format!(
                "cannot send {signal:?} activity signal: Gemini Live accepts activityStart/activityEnd \
                 only while automatic activity detection is disabled. Connect with \
                 `RealtimeConfig::without_vad()` to take ownership of turn detection."
            )));
        }

        // Claim the transition before sending, so two tasks reporting the same
        // barge-in produce one frame rather than racing to write two.
        let target = signal.target_state();
        if self
            .activity_open
            .compare_exchange(!target, target, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            tracing::debug!(
                session_id = %self.session_id,
                signal = ?signal,
                "activity signal suppressed: session is already in that state"
            );
            return Ok(ActivitySignalOutcome::Redundant);
        }

        let msg = GeminiClientMessage {
            realtime_input: Some(signal.into_realtime_input()),
            ..Default::default()
        };

        match self.send_raw(&msg).await {
            Ok(()) => Ok(ActivitySignalOutcome::Sent),
            Err(e) => {
                // The frame never reached the connection, so the session did
                // not make the transition. Leaving the flag claimed would
                // silently drop the caller's next, genuine signal.
                self.activity_open.store(!target, Ordering::SeqCst);
                Err(e)
            }
        }
    }

    /// Flush any buffered audio to the server.
    async fn flush_audio(&self) -> Result<()> {
        let audio_base64 = {
            let mut buffer = self.audio_buffer.lock();
            if !buffer.is_empty() {
                let encoded = base64::engine::general_purpose::STANDARD.encode(&*buffer);
                buffer.clear();
                Some(encoded)
            } else {
                None
            }
        };

        if let Some(b64) = audio_base64 {
            self.send_audio_base64(&b64).await?;
        }
        Ok(())
    }

    /// Send initial setup message with pre-compiled tools and provider-private resume handle.
    async fn send_setup_with_compiled_tools(
        &self,
        model: &str,
        config: RealtimeConfig,
        compiled_tools: Option<Vec<Value>>,
        private_resume_handle: Option<String>,
    ) -> Result<()> {
        let setup =
            Self::build_setup_message(model, config, compiled_tools, private_resume_handle)?;

        if self.activity_detection == ActivityDetection::Manual {
            tracing::warn!(
                session_id = %self.session_id,
                "Gemini server-side activity detection disabled; this session will not detect \
                 turns until the client sends activityStart/activityEnd via `activity_signaller()`"
            );
        }

        tracing::info!(model_id = %model, "Sending Gemini Live setup");
        self.send_raw(&setup).await
    }

    /// Build the `setup` frame for a config.
    fn build_setup_message(
        model: &str,
        config: RealtimeConfig,
        compiled_tools: Option<Vec<Value>>,
        private_resume_handle: Option<String>,
    ) -> Result<GeminiClientMessage<'_>> {
        let caps = capabilities_for(model);

        if !caps.configurable_thinking
            && let Some(extra) = &config.extra
            && extra.get("thinking_level").is_some()
        {
            return Err(RealtimeError::config(format!(
                "model '{model}' does not support configurable thinking"
            )));
        }

        if !caps.affective_dialog && config.affective_dialog == Some(true) {
            return Err(RealtimeError::config(format!(
                "model '{model}' does not support affective dialog"
            )));
        }

        if !caps.proactive_audio && config.proactive_audio == Some(true) {
            return Err(RealtimeError::config(format!(
                "model '{model}' does not support proactive audio"
            )));
        }

        if !caps.cached_content && config.cached_content.is_some() {
            return Err(RealtimeError::config(format!(
                "model '{model}' does not support cached content"
            )));
        }

        let effective_modalities = if caps.audio_only_response {
            if let Some(modalities) = &config.modalities {
                let has_audio = modalities.iter().any(|m| m.eq_ignore_ascii_case("audio"));
                if !has_audio {
                    return Err(RealtimeError::config(format!(
                        "model '{model}' requires AUDIO response modality, got '{modalities:?}'"
                    )));
                }
                if modalities.len() > 1 {
                    tracing::info!(
                        model,
                        ?modalities,
                        "model requires audio-only response; sanitizing responseModalities to [\"AUDIO\"]"
                    );
                }
                vec!["AUDIO".to_string()]
            } else {
                vec!["AUDIO".to_string()]
            }
        } else {
            gemini_response_modalities(config.modalities.as_deref())
        };

        let mut generation_config = json!({
            "responseModalities": effective_modalities,
        });

        if let Some(voice) = &config.voice {
            generation_config["speechConfig"] = json!({
                "voiceConfig": {
                    "prebuiltVoiceConfig": {
                        "voiceName": voice
                    }
                }
            });
        }

        // Spoken language (`RealtimeConfig::with_language`). Without it the model detects the
        // language per turn, and short or accented utterances are transcribed in the wrong one.
        // Read from the typed field; for one release a legacy
        // `extra.language_code` is still read (never written) when the
        // typed field is unset.
        let language_code =
            config.language.as_ref().map(|code| code.as_str().to_string()).or_else(|| {
                config
                    .extra
                    .as_ref()
                    .and_then(|ext| ext.get("language_code"))
                    .and_then(|val| val.as_str())
                    .and_then(|code| crate::config::LanguageCode::parse(code).ok())
                    .map(|code| code.as_str().to_string())
            });
        if let Some(code) = language_code {
            generation_config["speechConfig"]["languageCode"] = json!(code);
        }

        if let Some(temp) = config.temperature {
            generation_config["temperature"] = json!(temp);
        }

        // Emotion-aware ("affective") dialog — a generationConfig field, honored
        // by 3.8 Live models on the v1beta endpoint (the reference SDK nests
        // LiveConnectConfig.enable_affective_dialog here).
        if config.affective_dialog == Some(true) {
            generation_config["enableAffectiveDialog"] = json!(true);
        }

        if let Some(extra) = &config.extra
            && let Some(thinking_level) = extra.get("thinking_level")
            && let Some(obj) = generation_config.as_object_mut()
        {
            obj.insert("thinkingConfig".to_string(), json!({ "thinkingLevel": thinking_level }));
        }

        // Computed before anything moves out of `config`.
        let realtime_input_config = gemini_realtime_input_config(&config);
        let system_instruction = config.instruction.map(|text| GeminiContent {
            parts: vec![GeminiPart { text: Some(text), inline_data: None }],
        });

        let normalized_model = normalize_model_id(model);

        let context_window_compression =
            GeminiContextWindowCompression::from_extra(config.extra.as_ref());

        // Session resumption handle is strictly supplied by the Gemini adapter itself.
        // Resumption is on by default; `with_session_resumption(false)`
        // keeps `sessionResumption` out of the setup entirely.
        let session_resumption = if config.session_resumption == Some(false) {
            None
        } else {
            Some(SessionResumptionConfig { handle: private_resume_handle })
        };

        // When transcription is requested, enable both input (user speech) and
        // output (model speech) transcription so consumers get clean text for
        // native-audio turns. An empty object turns the feature on.
        let transcription = config.input_audio_transcription.as_ref().map(|_| json!({}));

        // Proactive audio filtering. The capability gate above already rejected
        // this knob on models the docs exclude, so reaching here with it set
        // means the model honors it.
        let proactivity = if config.proactive_audio == Some(true) {
            Some(json!({ "proactiveAudio": true }))
        } else {
            None
        };

        Ok(GeminiClientMessage {
            setup: Some(GeminiSetup {
                model: Some(normalized_model),
                system_instruction,
                generation_config: Some(generation_config),
                tools: compiled_tools,
                cached_content: config.cached_content,
                session_resumption,
                input_audio_transcription: transcription.clone(),
                output_audio_transcription: transcription,
                realtime_input_config,
                context_window_compression,
                proactivity,
            }),
            ..Default::default()
        })
    }

    /// Send a raw message to the writer task via local mpsc queue.
    ///
    /// # Admission & Delivery Semantics
    ///
    /// Returning `Ok(())` indicates only that the message was accepted by the
    /// local writer `mpsc` queue for asynchronous transmission.
    ///
    /// `Ok(())` **does NOT guarantee or prove**:
    /// - `WebSocket` socket-sink completion or TCP write
    /// - Network arrival or peer socket acceptance
    /// - Provider/Gemini service reception or protocol handling
    /// - Model turn execution or business-logic processing
    ///
    /// If local queue enqueue fails (e.g. channel closed due to writer exit),
    /// this method returns `RealtimeError::ConnectionError`.
    async fn send_raw<T: Serialize>(&self, value: &T) -> Result<()> {
        let msg = serde_json::to_string(value)
            .map_err(|e| RealtimeError::protocol(format!("JSON serialize error: {}", e)))?;

        self.outbound_tx
            .lock()
            .await
            .send(Message::Text(msg.into()))
            .await
            .map_err(|e| RealtimeError::connection(format!("Send queue error: {e}")))?;

        Ok(())
    }

    /// Receive and parse the next message.
    async fn receive_raw(&self) -> Option<Result<ServerEvent>> {
        // First check if there's anything in the queue
        {
            let mut queue = self.event_queue.lock();
            if let Some(event) = queue.pop_front() {
                return Some(Ok(event));
            }
        }

        let mut receiver = self.receiver.lock().await;

        loop {
            match receiver.next().await {
                Some(Ok(Message::Text(text))) => match self.translate_gemini_event(&text) {
                    Ok(events) => {
                        let mut queue = self.event_queue.lock();
                        let mut iter = events.into_iter();
                        if let Some(first) = iter.next() {
                            for event in iter {
                                queue.push_back(event);
                            }
                            return Some(Ok(first));
                        }
                        // Control frame produced no public events (e.g. non-resumable SessionResumptionUpdate); continue reading
                    }
                    Err(e) => return Some(Err(e)),
                },
                Some(Ok(Message::Binary(bytes))) => match String::from_utf8(bytes.to_vec()) {
                    Ok(text) => match self.translate_gemini_event(&text) {
                        Ok(events) => {
                            let mut queue = self.event_queue.lock();
                            let mut iter = events.into_iter();
                            if let Some(first) = iter.next() {
                                for event in iter {
                                    queue.push_back(event);
                                }
                                return Some(Ok(first));
                            }
                            // Control frame produced no public events; continue reading
                        }
                        Err(e) => return Some(Err(e)),
                    },
                    Err(e) => {
                        return Some(Err(RealtimeError::protocol(format!(
                            "Invalid UTF-8 in binary message: {}",
                            e
                        ))));
                    }
                },
                Some(Ok(Message::Close(close_frame))) => {
                    // Structured, because a server-initiated close and a dead
                    // transport both surface downstream as the same `None` and get
                    // the same terminal reason. The code and reason are the only
                    // thing distinguishing "the provider aborted an idle session"
                    // from "the network dropped", and `{:?}` on the whole frame
                    // buries both inside a Debug string nothing can filter on.
                    tracing::error!(
                        close_code = close_frame
                            .as_ref()
                            .map(|frame| u16::from(frame.code))
                            .unwrap_or_default(),
                        close_reason =
                            close_frame.as_ref().map(|frame| frame.reason.as_ref()).unwrap_or(""),
                        "WebSocket closed by server"
                    );
                    *self.last_disconnect.lock() = Some(crate::session::DisconnectReason {
                        code: close_frame.as_ref().map(|frame| u16::from(frame.code)),
                        reason: close_frame
                            .as_ref()
                            .map(|frame| frame.reason.to_string())
                            .unwrap_or_default(),
                    });
                    self.connected.store(false, Ordering::SeqCst);
                    return None;
                }
                Some(Ok(msg)) => {
                    tracing::warn!("Received unhandled tungstenite message: {:?}", msg);
                    return Some(Ok(ServerEvent::Unknown));
                }
                Some(Err(e)) => {
                    self.connected.store(false, Ordering::SeqCst);
                    return Some(Err(RealtimeError::connection(format!("Receive error: {}", e))));
                }
                None => {
                    self.connected.store(false, Ordering::SeqCst);
                    return None;
                }
            }
        }
    }

    /// Translate Gemini-specific events to unified format.
    ///
    /// Also latches a resumable `sessionResumptionUpdate` handle privately on
    /// `self.last_resume_handle` so that `reconnect_with_backoff` can present
    /// it on the next connection attempt; the handle never surfaces as a
    /// public event.
    pub(crate) fn translate_gemini_event(&self, raw: &str) -> Result<Vec<ServerEvent>> {
        // Parsed once here; the parsed value is handed to the translator
        // instead of being re-parsed from `raw` a second time per frame.
        let value: Value = serde_json::from_str(raw)
            .map_err(|e| RealtimeError::protocol(describe_unparseable_frame(raw, &e)))?;
        {
            let value = &value;
            if let Some(resumption_update) = value.get("sessionResumptionUpdate") {
                let resumable =
                    resumption_update.get("resumable").and_then(Value::as_bool).unwrap_or(false);
                if resumable
                    && let Some(handle) = resumption_update
                        .get("newHandle")
                        .and_then(Value::as_str)
                        .filter(|handle| !handle.trim().is_empty())
                {
                    *self.last_resume_handle.lock() =
                        Some((handle.to_owned(), std::time::Instant::now()));
                    tracing::debug!(
                        resume_checkpoint_observed = true,
                        "Stored resumable Gemini session handle for reconnect"
                    );
                }
            }

            if let Some(go_away) = value.get("goAway")
                && let Some(time_left_str) = go_away.get("timeLeft").and_then(Value::as_str)
            {
                if let Some(duration) = parse_duration_string(time_left_str) {
                    let deadline = std::time::Instant::now() + duration;
                    let _ = self.replacement_deadline_tx.send(Some(deadline));
                    tracing::info!(
                        deadline = ?deadline,
                        "Broadcasted planned replacement deadline from Gemini goAway frame"
                    );
                } else {
                    tracing::warn!(
                        raw_time_left = time_left_str,
                        "Ignored unparseable timeLeft in Gemini goAway frame"
                    );
                }
            }
        }

        let events = self.translate_event_logged(value, Some(&self.frame_log))?;
        for event in &events {
            match event {
                // Latched, never cleared: a session that completed setup once
                // has proved the configuration is acceptable, and a later close
                // is about the connection rather than the request.
                ServerEvent::SessionCreated { .. } => {
                    self.setup_complete_seen.store(true, Ordering::SeqCst);
                }
                ServerEvent::FunctionCallDone { call_id, name, .. } => {
                    self.call_names.lock().insert(call_id.clone(), name.clone());
                }
                ServerEvent::ToolCallCancelled { call_ids } => {
                    let mut names = self.call_names.lock();
                    for id in call_ids {
                        names.remove(id.as_str());
                    }
                }
                _ => {}
            }
        }
        Ok(events)
    }

    /// Returns the current number of tracked function call names (test helper).
    #[cfg(test)]
    pub fn call_names_count(&self) -> usize {
        self.call_names.lock().len()
    }

    /// Test entry point: no session, so no per-connection telemetry identity, no
    /// speech-onset latch, and no usage stash. `voiceActivity` still translates —
    /// it carries its own offset — but an `interrupted` never synthesizes a
    /// `SpeechStarted` the static path could not dedup against a prior onset.
    pub fn translate_event_static(raw: &str) -> Result<Vec<ServerEvent>> {
        let value: Value = serde_json::from_str(raw)
            .map_err(|e| RealtimeError::protocol(describe_unparseable_frame(raw, &e)))?;
        log_frame_shape(&value, None);
        if let Some(activity) = voice_activity_event(&value) {
            let rest = Self::translate_message_core(&value, true)?;
            return Ok(std::iter::once(activity)
                .chain(rest.into_iter().filter(|event| !matches!(event, ServerEvent::Unknown)))
                .collect());
        }
        Self::translate_message_core(&value, true)
    }

    /// Surface top-level `usageMetadata` as a [`ServerEvent::Usage`].
    ///
    /// Returns `None` when the frame carries no usage block. A present but
    /// mistyped block is a protocol error, like a malformed
    /// `toolCallCancellation`: the counts feed cost proof, and silently
    /// dropping billing data is worse than a visible error.
    fn parse_usage_event(value: &Value) -> Result<Option<ServerEvent>> {
        let Some(raw) = value.get("usageMetadata") else {
            return Ok(None);
        };
        let usage: GeminiUsageMetadata = serde_json::from_value(raw.clone())
            .map_err(|e| RealtimeError::protocol(format!("Malformed usageMetadata: {e}")))?;
        Ok(Some(ServerEvent::Usage {
            event_id: uuid::Uuid::new_v4().to_string(),
            usage: usage.into(),
        }))
    }

    pub(crate) fn translate_event_logged(
        &self,
        value: Value,
        frame_log: Option<&FrameLog>,
    ) -> Result<Vec<ServerEvent>> {
        log_frame_shape(&value, frame_log);

        // `voiceActivity` is outside the message's union, so it arrives beside the
        // `serverContent.speechState` that carries nothing else.
        let Some(activity) = voice_activity_event(&value) else {
            return self.translate_gemini_message(value);
        };
        self.speech_active
            .store(matches!(activity, ServerEvent::SpeechStarted { .. }), Ordering::SeqCst);
        let rest = self.translate_gemini_message(value)?;
        Ok(std::iter::once(activity)
            .chain(rest.into_iter().filter(|event| !matches!(event, ServerEvent::Unknown)))
            .collect())
    }

    /// Translate one parsed Gemini server message, apart from its `voiceActivity`.
    ///
    /// The stateful wrapper around [`Self::translate_message_core`]: it dedups the
    /// synthesized barge-in onset against the speech latch, and it stashes usage
    /// reports that belong to a response that has not closed yet so they attach to
    /// the `ResponseDone` that closes it instead of surfacing twice — once now and
    /// once attached. A stashed report that leaves nothing else behind reads as
    /// [`ServerEvent::Unknown`], the translator's signal for "no event on this frame".
    fn translate_gemini_message(&self, value: Value) -> Result<Vec<ServerEvent>> {
        let has_content = value.get("serverContent").is_some();
        let turn_complete = value
            .get("serverContent")
            .and_then(|c| c.get("turnComplete"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let has_usage = value.get("usageMetadata").is_some();
        // A report that belongs to a response that closes later: stashed, not emitted.
        // `turnComplete` with inline counts is reported immediately by the core.
        let defer_usage = has_usage
            && ((has_content && !turn_complete)
                || (!has_content && value.get("toolCall").is_none()));
        if defer_usage && let Some(usage) = value.get("usageMetadata") {
            *self.last_usage.lock() = Some(usage.clone());
        }

        let speech_already_reported = self.speech_active.load(Ordering::SeqCst);
        let mut events = Self::translate_message_core(&value, speech_already_reported)?;
        if !speech_already_reported
            && events.iter().any(|event| matches!(event, ServerEvent::SpeechStarted { .. }))
        {
            // The core synthesized this onset from an `interrupted` frame (no
            // `voiceActivity` onset preceded it). Latch it, so a repeated
            // interrupted frame does not report the same speech twice; the
            // `voiceActivity` ACTIVITY_END store clears the latch.
            self.speech_active.store(true, Ordering::SeqCst);
        }

        if defer_usage {
            events.retain(|event| !matches!(event, ServerEvent::Usage { .. }));
            if events.is_empty() {
                events.push(ServerEvent::Unknown);
            }
        } else if turn_complete
            && !has_usage
            && let Some(usage) = self.last_usage.lock().take()
            && let Some(response) = events.iter_mut().find_map(|event| match event {
                ServerEvent::ResponseDone { response, .. } => Some(response),
                _ => None,
            })
            && response.get("usageMetadata").is_none()
        {
            // A report that arrived on its own just before this turn closed belongs
            // to the response that closes here.
            response["usageMetadata"] = usage;
        }
        Ok(events)
    }

    /// Stateless translation of one parsed Gemini server message, shared by the
    /// session path and the static test path.
    ///
    /// `speech_already_reported` suppresses the synthesized barge-in onset: the
    /// session passes its speech latch, the static path passes `true` because it
    /// has no latch to dedup against. Usage reports always surface immediately;
    /// deferring them to a closing response is the session wrapper's job.
    fn translate_message_core(
        value: &Value,
        speech_already_reported: bool,
    ) -> Result<Vec<ServerEvent>> {
        // Check for setup completion
        if let Some(_setup_complete) = value.get("setupComplete") {
            return Ok(vec![ServerEvent::SessionCreated {
                event_id: uuid::Uuid::new_v4().to_string(),
                session: (*value).clone(),
            }]);
        }

        // Check for goAway planned rotation signal
        if let Some(go_away) = value.get("goAway") {
            let time_left = go_away.get("timeLeft").and_then(Value::as_str).map(str::to_owned);
            tracing::info!(time_left = ?time_left, "gemini live goAway frame received");
            return Ok(vec![ServerEvent::PlannedRotation { time_left }]);
        }

        // Check for server content (audio/text)
        if let Some(content) = value.get("serverContent") {
            let mut events = Vec::new();

            // Barge-in: the caller spoke over the model, so this generation is
            // abandoned and the client must empty its playback queue.
            let interrupted = content.get("interrupted").and_then(Value::as_bool).unwrap_or(false);
            if interrupted {
                // The user's speech cut the model's reply off: the barge-in onset
                // OpenAI sends as `speech_started`. Without a `voiceActivity` onset
                // there is no offset, so `audio_start_ms` is 0; an onset the
                // server already reported is reported once.
                if !speech_already_reported {
                    events.push(ServerEvent::SpeechStarted {
                        event_id: uuid::Uuid::new_v4().to_string(),
                        audio_start_ms: 0,
                    });
                }
                events.push(ServerEvent::ResponseCancelled {
                    event_id: uuid::Uuid::new_v4().to_string(),
                });
            }

            // `modelTurn` content in an interrupted frame belongs to the
            // generation that was just abandoned — a new generation only starts
            // in response to later client input. Translating it would hand the
            // consumer a purge immediately followed by one last stale chunk to
            // enqueue, which is the very audio the purge exists to remove.
            //
            // Only playback-bearing content is suppressed. `inputTranscription`
            // below is the *caller's* speech and is ordered independently of
            // model-turn messages, so it must survive.
            if !interrupted
                && let Some(parts) = content.get("modelTurn").and_then(|t| t.get("parts"))
                && let Some(parts_arr) = parts.as_array()
            {
                for part in parts_arr {
                    // Audio output — decode base64 to raw bytes
                    if let Some(inline_data) = part.get("inlineData")
                        && let Some(data) = inline_data.get("data").and_then(|d| d.as_str())
                    {
                        let decoded = base64::engine::general_purpose::STANDARD
                            .decode(data)
                            .unwrap_or_default();
                        events.push(ServerEvent::AudioDelta {
                            event_id: uuid::Uuid::new_v4().to_string(),
                            response_id: String::new(),
                            item_id: String::new(),
                            output_index: 0,
                            content_index: 0,
                            delta: decoded,
                        });
                    }
                    // Text output
                    if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                        events.push(ServerEvent::TextDelta {
                            event_id: uuid::Uuid::new_v4().to_string(),
                            response_id: String::new(),
                            item_id: String::new(),
                            output_index: 0,
                            content_index: 0,
                            delta: text.to_string(),
                        });
                    }
                }
            }

            // Output transcription: the model's spoken words as text (enabled via
            // outputAudioTranscription). Surfaced as a transcript delta so it
            // reads the same as OpenAI's audio transcript.
            if let Some(text) = content
                .get("outputTranscription")
                .and_then(|o| o.get("text"))
                .and_then(|t| t.as_str())
                && !text.is_empty()
            {
                events.push(ServerEvent::TranscriptDelta {
                    event_id: uuid::Uuid::new_v4().to_string(),
                    response_id: String::new(),
                    item_id: String::new(),
                    output_index: 0,
                    content_index: 0,
                    delta: text.to_string(),
                });
            }

            // Input transcription: the user's speech as text (streamed in chunks).
            if let Some(text) = content
                .get("inputTranscription")
                .and_then(|o| o.get("text"))
                .and_then(|t| t.as_str())
                && !text.is_empty()
            {
                events.push(ServerEvent::InputTranscriptDelta {
                    item_id: String::new(),
                    content_index: 0,
                    delta: text.to_string(),
                });
            }

            if let Some(turn_complete) = content.get("turnComplete")
                && turn_complete.as_bool().unwrap_or(false)
            {
                // Attaching a stashed report from an earlier frame is the session
                // wrapper's job; the core reports what rides on this frame.
                events.push(ServerEvent::ResponseDone {
                    event_id: uuid::Uuid::new_v4().to_string(),
                    response: (*value).clone(),
                });
            }

            // Turn-boundary signals the translator previously dropped: the
            // model is waiting for input, content generation ended, and the
            // server's interaction state. Booleans and enums only — like the
            // frame-shape log, these must never carry transcript text.
            if let Some(waiting) = content.get("waitingForInput")
                && waiting.as_bool().unwrap_or(false)
            {
                events.push(ServerEvent::ModelWaitingForInput {
                    event_id: uuid::Uuid::new_v4().to_string(),
                });
            }

            if let Some(done) = content.get("generationComplete")
                && done.as_bool().unwrap_or(false)
            {
                events.push(ServerEvent::GenerationComplete {
                    event_id: uuid::Uuid::new_v4().to_string(),
                });
            }

            if let Some(raw) = content.get("interactionStatus").and_then(Value::as_str) {
                let status = match raw {
                    "IDLE" => InteractionStatus::Idle,
                    "IN_PROGRESS" => InteractionStatus::InProgress,
                    _ => {
                        tracing::debug!(
                            interaction_status = raw,
                            "unknown interactionStatus; surfacing as Unknown"
                        );
                        InteractionStatus::Unknown
                    }
                };
                events.push(ServerEvent::InteractionStatus {
                    event_id: uuid::Uuid::new_v4().to_string(),
                    status,
                });
            }

            // `usageMetadata` sits outside the `messageType` union: a
            // turn-final `serverContent` frame typically carries the turn's
            // token counts alongside `turnComplete`. Surface them here so the
            // turn's content and its counts arrive together.
            if let Some(usage) = Self::parse_usage_event(value)? {
                events.push(usage);
            }

            if !events.is_empty() {
                return Ok(events);
            }
        }

        // `sessionResumptionUpdate` carries `newHandle` and `resumable`. Only a
        // `resumable: true` update names a safe resume point: Google specifies a
        // handle is usable only when it is true, so an update with
        // `resumable: false` ("no safe resume point right now") must not
        // overwrite the last handle that *was* safe. The session wrapper latches
        // the handle privately for reconnect; the translator itself emits no
        // event for it.
        if let Some(resumption_update) = value.get("sessionResumptionUpdate") {
            let resumable =
                resumption_update.get("resumable").and_then(|r| r.as_bool()).unwrap_or(false);
            let handle = resumption_update
                .get("newHandle")
                .and_then(|h| h.as_str())
                .filter(|handle| !handle.trim().is_empty());
            match (resumable, handle) {
                (true, Some(_handle)) => {
                    tracing::debug!(
                        resume_checkpoint_observed = true,
                        "Received a resumable sessionResumptionUpdate; handle stored privately"
                    );
                    return Ok(vec![]);
                }
                _ => {
                    tracing::debug!(
                        resumable,
                        has_handle = handle.is_some(),
                        "Ignoring a non-resumable sessionResumptionUpdate"
                    );
                    return Ok(vec![]);
                }
            }
        }

        // Gemini withdraws function calls it already issued, normally because
        // the caller interrupted the turn that produced them. Google's wording
        // is that they "should not have been executed and should be cancelled",
        // and that a client which already caused side effects may need to undo
        // them — so dropping this frame means a request the caller withdrew can
        // still take effect.
        if let Some(raw) = value.get("toolCallCancellation") {
            let cancellation: GeminiToolCallCancellation = serde_json::from_value(raw.clone())
                .map_err(|e| {
                    RealtimeError::protocol(format!("Malformed toolCallCancellation: {e}"))
                })?;

            let call_ids: Vec<crate::events::ToolCallId> = cancellation
                .ids
                .into_iter()
                .filter_map(|id| match crate::events::ToolCallId::parse(id) {
                    Ok(id) => Some(id),
                    Err(_) => {
                        // An id that identifies nothing is worse than useless:
                        // it would let a consumer report a cancellation as
                        // handled when it matched no outstanding call.
                        tracing::warn!("Gemini sent an empty toolCallCancellation id; ignoring it");
                        None
                    }
                })
                .collect();
            if !call_ids.is_empty() {
                return Ok(vec![ServerEvent::ToolCallCancelled { call_ids }]);
            }
        }

        // Check for tool calls
        if let Some(tool_call) = value.get("toolCall")
            && let Some(calls) = tool_call.get("functionCalls").and_then(|c| c.as_array())
            && !calls.is_empty()
        {
            // Gemini batches parallel function calls in one frame; emit one
            // event per call — dropping any leaves the model waiting forever
            // for the missing function response.
            return calls
                .iter()
                .enumerate()
                .map(|(idx, call)| {
                    let name = call.get("name").and_then(|n| n.as_str()).ok_or_else(|| {
                        RealtimeError::protocol("Gemini tool call missing 'name'")
                    })?;
                    let id = call
                        .get("id")
                        .and_then(|i| i.as_str())
                        .ok_or_else(|| RealtimeError::protocol("Gemini tool call missing 'id'"))?;
                    let args = call.get("args").cloned().ok_or_else(|| {
                        RealtimeError::protocol("Gemini tool call missing 'args'")
                    })?;

                    if !args.is_object() {
                        return Err(RealtimeError::protocol(format!(
                            "Gemini tool call 'args' must be an object, got: {args:?}"
                        )));
                    }

                    Ok(ServerEvent::FunctionCallDone {
                        event_id: uuid::Uuid::new_v4().to_string(),
                        response_id: String::new(),
                        item_id: String::new(),
                        output_index: idx as u32,
                        call_id: id.to_string(),
                        name: name.to_string(),
                        arguments: args,
                    })
                })
                .collect();
        }

        // Standalone usage frame. Usage riding `serverContent` is handled in
        // that branch above; anything reaching here alongside usage is a
        // control frame this translator otherwise ignores, so the usage is
        // the whole signal.
        if let Some(usage) = Self::parse_usage_event(value)? {
            return Ok(vec![usage]);
        }

        Ok(vec![ServerEvent::Unknown])
    }
}

#[async_trait]
impl RealtimeRecovery for GeminiRealtimeSession {
    fn classify(&self, cause: &RecoveryCause) -> RecoveryDisposition {
        match cause {
            RecoveryCause::ReadFailed(err) | RecoveryCause::WriteFailed(err) => {
                if err.is_connection_reset() {
                    RecoveryDisposition::Recoverable
                } else {
                    RecoveryDisposition::Fatal
                }
            }
            // The server closed the connection, deliberately, before it ever
            // accepted our setup. That is a refusal of the setup frame, and
            // `recover` rebuilds the candidate from the same effective
            // configuration -- so the retry re-sends the exact request that
            // was just refused.
            //
            // Both halves of this condition carry weight. `setup_complete_seen`
            // separates a refusal from a transport that died mid-session; 1008
            // covers both, so the close code cannot decide it alone. And
            // `last_disconnect` is the evidence that the provider actually
            // spoke: `UnexpectedEof` reaches here from two transports, a real
            // `Message::Close` (which records a code and reason) and a stream
            // that simply ended (which records nothing). Only the first is
            // evidence of a rejected configuration. A bare disappearance before
            // setup is exactly the transient fault managed recovery exists for,
            // and it stays recoverable.
            //
            // Failing fast on the refusal is what keeps `disconnect_reason()`
            // pointing at the close the server sent, instead of the generic
            // "closed before setupComplete" the doomed retry would overwrite it
            // with -- and that reason is the only thing that says whether the
            // model ID, the tool schema, or the quota was at fault.
            RecoveryCause::UnexpectedEof
                if !self.setup_complete_seen.load(Ordering::SeqCst)
                    && self.last_disconnect.lock().is_some() =>
            {
                RecoveryDisposition::Fatal
            }
            RecoveryCause::UnexpectedEof | RecoveryCause::PlannedRotation { .. } => {
                RecoveryDisposition::Recoverable
            }
        }
    }

    fn classify_attempt_error(&self, error: &RealtimeError) -> RecoveryDisposition {
        if error.is_connection_reset() {
            RecoveryDisposition::Recoverable
        } else {
            RecoveryDisposition::Fatal
        }
    }

    async fn recover(&self, context: RecoveryContext<'_>) -> Result<RecoveredSession> {
        let deadline = context.deadline();
        let config = context.config();
        let prior_handle = self.fresh_resume_handle();

        struct CandidateGuard {
            session: Option<Arc<GeminiRealtimeSession>>,
        }

        impl CandidateGuard {
            fn new(session: GeminiRealtimeSession) -> Self {
                Self { session: Some(Arc::new(session)) }
            }

            fn disarm(mut self) -> Arc<GeminiRealtimeSession> {
                self.session.take().expect("candidate exists")
            }
        }

        impl Drop for CandidateGuard {
            fn drop(&mut self) {
                if let Some(session) = self.session.take() {
                    session.connected.store(false, Ordering::SeqCst);
                    if let Ok(token) = session.cancel_token.try_lock() {
                        token.cancel();
                    }
                    if let Ok(mut writer) = session.writer_task.try_lock()
                        && let Some(handle) = writer.take()
                    {
                        handle.abort();
                    }
                }
            }
        }

        // One attempt presenting `handle` (or none) in its setup.
        let make_attempt = |handle: Option<String>| {
            let backend = self.backend.clone();
            let model = self.reconnect_model.clone();
            let attempt_config = config.clone();
            let dialect = self.dialect;
            async move {
                let prior_handle = handle;
                let config = attempt_config;
                let candidate_session = Self::connect_with_dialect_and_resume(
                    backend,
                    &model,
                    config.clone(),
                    dialect,
                    prior_handle.clone(),
                )
                .await?;

                let guard = CandidateGuard::new(candidate_session);

                // Wait for setupComplete frame as the setup-first transaction
                loop {
                    match guard.session.as_ref().unwrap().next_event().await {
                        Some(Ok(ServerEvent::SessionCreated { .. })) => {
                            break;
                        }
                        Some(Ok(_)) => continue,
                        Some(Err(e)) => return Err(e),
                        None => {
                            return Err(RealtimeError::connection(
                                "Candidate connection closed before setupComplete frame",
                            ));
                        }
                    }
                }

                let candidate = guard.disarm();

                // Carry prior resume handle if candidate hasn't received a new one yet
                if candidate.last_resume_handle().is_none()
                    && let Some(handle) = &prior_handle
                {
                    *candidate.last_resume_handle.lock() =
                        Some((handle.clone(), std::time::Instant::now()));
                }

                let recovered: Arc<dyn RealtimeSession> = candidate;
                Ok(RecoveredSession::new(recovered, RecoveryContinuity::Reconnected))
            }
        };

        let deadline_instant = tokio::time::Instant::from_std(deadline);
        let first =
            tokio::time::timeout_at(deadline_instant, make_attempt(prior_handle.clone())).await;
        let first = match first {
            Ok(result) => result,
            Err(_) => Err(RealtimeError::Timeout(
                "Gemini recovery attempt timed out waiting for setupComplete".to_string(),
            )),
        };
        match first {
            Ok(recovered) => Ok(recovered),
            Err(error) if prior_handle.is_some() => {
                // A handle the provider will not resume from fails the
                // setup; retry exactly once with no handle presented.
                tracing::warn!(
                    error = crate::recovery::error_category(&error),
                    "Gemini recovery with a presented resumption handle failed; retrying once with no handle"
                );
                match tokio::time::timeout_at(deadline_instant, make_attempt(None)).await {
                    Ok(result) => result,
                    Err(_) => Err(RealtimeError::Timeout(
                        "Gemini recovery attempt timed out waiting for setupComplete".to_string(),
                    )),
                }
            }
            Err(error) => Err(error),
        }
    }
}

impl RealtimeLifecycle for GeminiRealtimeSession {
    fn subscribe_replacement_deadline(
        &self,
    ) -> tokio::sync::watch::Receiver<Option<std::time::Instant>> {
        self.replacement_deadline_rx.clone()
    }
}

#[async_trait]
impl RealtimeSession for GeminiRealtimeSession {
    fn lifecycle(&self) -> Option<&dyn RealtimeLifecycle> {
        Some(self)
    }

    fn recovery(&self) -> Option<&dyn RealtimeRecovery> {
        Some(self)
    }

    #[cfg(any(test, feature = "recovery-test-utils"))]
    async fn force_transport_break(&self) -> Result<()> {
        self.connected.store(false, Ordering::SeqCst);
        self.cancel_token.lock().await.cancel();
        let mut writer_task = self.writer_task.lock().await;
        if let Some(handle) = writer_task.take() {
            handle.abort();
        }
        Ok(())
    }

    fn session_id(&self) -> &str {
        &self.session_id
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    fn disconnect_reason(&self) -> Option<crate::session::DisconnectReason> {
        self.last_disconnect.lock().clone()
    }

    async fn send_audio(&self, audio: &AudioChunk) -> Result<()> {
        // Support both native PCM16/16kHz and G.711 μ-law (telephony) formats.
        let (pcm_data, format) = match &audio.format {
            f if f == &AudioFormat::pcm16_16khz() => (audio.data.clone(), f.clone()),
            f if f.encoding == crate::audio::AudioEncoding::G711Ulaw && f.sample_rate == 8000 => {
                // Upsample G.711 μ-law (8kHz) to PCM16 (16kHz) for the adapter's current contract.
                let samples_8khz = crate::audio::g711::decode_ulaw_frame(&audio.data);
                let mut samples_16khz = Vec::with_capacity(samples_8khz.len() * 2);
                for &sample in &samples_8khz {
                    samples_16khz.push(sample);
                    samples_16khz.push(sample);
                }
                let chunk =
                    AudioChunk::from_i16_samples(&samples_16khz, AudioFormat::pcm16_16khz());
                (chunk.data, AudioFormat::pcm16_16khz())
            }
            _ => {
                return Err(RealtimeError::config(format!(
                    "this Gemini adapter currently supports only pcm16 at 16kHz mono or g711_ulaw at 8kHz, received: {:?}",
                    audio.format
                )));
            }
        };

        // Format-aware threshold (sample rate/channels/bit depth), avoids hardcoded 16k assumptions.
        let flush_threshold_bytes = Self::flush_threshold_bytes(&format);

        // Path A: The incoming chunk already meets the batching threshold.
        // Send it immediately (after flushing any existing buffer) to avoid extra copies.
        if pcm_data.len() >= flush_threshold_bytes {
            self.flush_audio().await?;
            // Encode to base64 synchronously *before* awaiting to minimize lock/hold time.
            let audio_base64 = base64::engine::general_purpose::STANDARD.encode(&pcm_data);
            return self.send_audio_base64(&audio_base64).await;
        }

        // Path B: Smart Audio Buffering for small chunks.
        let audio_base64 = {
            let mut buffer = self.audio_buffer.lock();
            buffer.put_slice(&pcm_data);

            if buffer.len() >= flush_threshold_bytes {
                let encoded = base64::engine::general_purpose::STANDARD.encode(&*buffer);
                buffer.clear();
                Some(encoded)
            } else {
                None
            }
        };

        if let Some(b64) = audio_base64 {
            self.send_audio_base64(&b64).await?;
        }
        Ok(())
    }

    async fn send_audio_base64(&self, audio_base64: &str) -> Result<()> {
        let msg = GeminiClientMessage {
            realtime_input: Some(GeminiRealtimeInput {
                audio: Some(GeminiMediaChunk {
                    // Gemini Live requires raw 16-bit PCM little-endian at 16 kHz;
                    // declaring the rate removes any server-side ambiguity.
                    mime_type: "audio/pcm;rate=16000",
                    data: audio_base64,
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        self.send_raw(&msg).await
    }

    async fn send_video_frame(&self, mime_type: &str, data_base64: &str) -> Result<()> {
        self.send_raw(&video_frame_message(mime_type, data_base64)).await
    }

    async fn send_text(&self, text: &str) -> Result<()> {
        // Gemini 3.8 Live streams conversational user input as
        // `realtimeInput.text`; `clientContent` is reserved for injecting
        // context turns, so a user turn sent there would be read as
        // supplied context rather than as the user's next input.
        let msg = GeminiClientMessage {
            realtime_input: Some(GeminiRealtimeInput {
                text: Some(text.to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        self.send_raw(&msg).await
    }

    async fn send_tool_response(&self, response: ToolResponse) -> Result<()> {
        let output = match &response.output {
            Value::String(s) => json!({ "result": s }),
            other => other.clone(),
        };

        let name = self.call_names.lock().remove(&response.call_id).ok_or_else(|| {
            RealtimeError::protocol(format!(
                "Missing tool name for call_id '{}' in GeminiFunctionResponse",
                response.call_id
            ))
        })?;

        let msg = GeminiClientMessage {
            tool_response: Some(GeminiToolResponse {
                function_responses: vec![GeminiFunctionResponse {
                    id: response.call_id,
                    name,
                    response: output,
                    scheduling: response.scheduling,
                }],
            }),
            ..Default::default()
        };
        self.send_raw(&msg).await
    }

    async fn begin_activity(&self) -> Result<()> {
        // Silent in automatic mode rather than an error. The server owns
        // detection there, so a caller reporting caller-speech is telling us
        // something we already have a better source for — redundant, not
        // wrong, and failing it would punish a caller for using the neutral
        // API on a backend that happens not to need it.
        if self.activity_detection != ActivityDetection::Manual {
            return Ok(());
        }

        // The same frame `interrupt` sends, deliberately. Gemini opens a turn
        // and abandons the model's response with one signal, so both callers
        // converge here; `send_activity`'s edge de-duplication means two
        // reports of one turn-start still put one frame on the wire.
        let outcome = self.send_activity(ActivitySignal::Start).await?;
        tracing::debug!(
            session_id = %self.session_id,
            ?outcome,
            "begin activity sent activityStart"
        );
        Ok(())
    }

    async fn commit_audio(&self) -> Result<()> {
        self.flush_audio().await?;
        // Under automatic detection the server caches the tail of the stream and closes the
        // turn only once more audio or `audioStreamEnd` arrives; a client that has stopped
        // sending would otherwise leave the user's turn open indefinitely.
        if self.activity_detection == ActivityDetection::Automatic {
            let msg = GeminiClientMessage {
                realtime_input: Some(GeminiRealtimeInput {
                    audio_stream_end: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            };
            self.send_raw(&msg).await?;
        }

        // `commit_audio` is the provider-neutral boundary for a manually
        // delimited input turn. Gemini has no audio-buffer commit frame: once
        // automatic activity detection is disabled, its equivalent is an
        // `activityEnd` after every queued audio frame has been written.
        if self.activity_detection == ActivityDetection::Manual {
            let outcome = self.send_activity(ActivitySignal::End).await?;
            tracing::debug!(
                session_id = %self.session_id,
                ?outcome,
                "commit audio sent activityEnd"
            );
        }

        Ok(())
    }

    async fn clear_audio(&self) -> Result<()> {
        let mut buffer = self.audio_buffer.lock();
        buffer.clear();
        Ok(())
    }

    async fn create_response(&self) -> Result<()> {
        Ok(()) // Gemini auto-generates responses
    }

    async fn interrupt(&self) -> Result<()> {
        // Either way, drop audio we buffered but never framed: it belongs to
        // the turn being abandoned.
        self.clear_audio().await?;

        match self.activity_detection {
            ActivityDetection::Manual => {
                // With automatic detection disabled the server has no way to
                // learn the user has taken the turn, so `activityStart` is the
                // only thing that can carry a barge-in. Google documents it as
                // interrupting the model's response under the default
                // `activityHandling`; what is asserted here is that the frame
                // was sent, not what the server did with it.
                let outcome = self.send_activity(ActivitySignal::Start).await?;
                tracing::debug!(
                    session_id = %self.session_id,
                    ?outcome,
                    "interrupt sent activityStart"
                );
                Ok(())
            }
            ActivityDetection::Automatic => {
                // Deliberately not an error: this is the default configuration
                // and an interruption really is performed here, by the
                // server's own VAD on speech onset. What this call cannot do
                // is *initiate* one — Gemini Live has no client-side response
                // cancel, and `activityStart` is rejected while server-side
                // detection is enabled. So nothing goes to the server, and the
                // log says so rather than the return value implying otherwise.
                tracing::debug!(
                    session_id = %self.session_id,
                    "interrupt sent nothing to the server: Gemini's server-side activity detection \
                     owns barge-in in this configuration, and client activity frames are only legal \
                     with it disabled. Only unsent local audio was discarded."
                );
                Ok(())
            }
        }
    }

    async fn send_event(&self, event: ClientEvent) -> Result<()> {
        match event {
            // Intercept standard messages from the orchestrator
            ClientEvent::Message { role, parts } => {
                let msg = translate_client_message(&role, parts);
                tracing::info!(role = ?role, "Injecting mid-flight context via native adk-rust types");
                self.send_raw(&msg).await
            }

            // Explicitly handle all other unified ClientEvent variants.
            // Returning Ok(()) silently for unsupported features is an anti-pattern.
            // We log a clear warning that Gemini does not natively support this specific control event.
            ClientEvent::AudioDelta { .. } => {
                tracing::warn!(
                    "AudioDelta is explicitly handled via `send_audio`, not `send_event`. Dropping event."
                );
                Ok(())
            }
            ClientEvent::InputAudioBufferCommit => {
                tracing::warn!(
                    "Gemini Live API does not support manual audio buffer commits. Dropping event."
                );
                Ok(())
            }
            ClientEvent::InputAudioBufferClear => {
                tracing::warn!(
                    "Gemini Live API does not support manual audio buffer clears via wire events. Dropping event."
                );
                Ok(())
            }
            ClientEvent::ConversationItemCreate { .. } => {
                tracing::warn!(
                    "Raw ConversationItemCreate is an OpenAI construct. Use ClientEvent::Message instead for Gemini. Dropping event."
                );
                Ok(())
            }
            ClientEvent::ResponseCreate { .. } => {
                tracing::warn!(
                    "Gemini Live API automatically generates responses based on VAD/turns. Manual ResponseCreate is unsupported. Dropping event."
                );
                Ok(())
            }
            ClientEvent::ResponseCancel => match self.activity_detection {
                // Actionable here: with server-side detection off, `activityStart`
                // is the cancel, so honour the event instead of dropping it.
                ActivityDetection::Manual => self.interrupt().await,
                ActivityDetection::Automatic => {
                    tracing::warn!(
                        "Gemini Live API has no client-side response cancel while server-side \
                         activity detection is enabled; it interrupts on its own VAD. Connect with \
                         `RealtimeConfig::without_vad()` to drive interruption from the client. \
                         Dropping event."
                    );
                    Ok(())
                }
            },
            ClientEvent::SessionUpdate { .. } => {
                tracing::warn!(
                    "Raw SessionUpdate is an OpenAI construct. Use RealtimeRunner's `update_session` for provider-agnostic Context Mutation. Dropping event."
                );
                Ok(())
            }
            ClientEvent::UpdateSession { .. } => {
                tracing::error!(
                    "Internal UpdateSession intent leaked to the Gemini transport socket. This should have been intercepted by the RealtimeRunner."
                );
                Err(RealtimeError::ProviderError("Internal intent leaked to transport".to_string()))
            }
        }
    }

    async fn next_event(&self) -> Option<Result<ServerEvent>> {
        self.receive_raw().await
    }

    fn events(&self) -> Pin<Box<dyn Stream<Item = Result<ServerEvent>> + Send + '_>> {
        Box::pin(async_stream::stream! {
            while self.is_connected() {
                match self.receive_raw().await {
                    Some(Ok(event)) => yield Ok(event),
                    Some(Err(e)) => yield Err(e),
                    None => break,
                }
            }
        })
    }

    async fn close(&self) -> Result<()> {
        self.connected.store(false, Ordering::SeqCst);

        // Attempt graceful close frame enqueue under a short timeout (500ms) so a full
        // channel never blocks teardown indefinitely.
        let _ = tokio::time::timeout(std::time::Duration::from_millis(500), async {
            self.outbound_tx.lock().await.send(Message::Close(None)).await
        })
        .await;

        // Signal cancellation token to wake writer task if blocked on channel read or network I/O
        self.cancel_token.lock().await.cancel();

        // Ensure deterministic teardown: await writer task completion under 1s timeout, aborting if stalled
        let mut writer_task = self.writer_task.lock().await;
        if let Some(mut handle) = writer_task.take()
            && tokio::time::timeout(std::time::Duration::from_secs(1), &mut handle).await.is_err()
        {
            tracing::warn!("Gemini writer task did not exit within 1s; aborting handle");
            handle.abort();
        }
        Ok(())
    }

    async fn mutate_context(
        &self,
        config: crate::config::RealtimeConfig,
    ) -> Result<ContextMutationOutcome> {
        tracing::info!(
            "Gemini API does not support native mid-flight context swaps; signalling resumption needed."
        );
        Ok(ContextMutationOutcome::RequiresResumption(Box::new(config)))
    }
}

impl std::fmt::Debug for GeminiRealtimeSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeminiRealtimeSession")
            .field("session_id", &self.session_id)
            .field("connected", &self.connected.load(Ordering::SeqCst))
            .finish()
    }
}

/// Maps provider-neutral modality names onto Gemini's `Modality` enum.
///
/// `RealtimeConfig::modalities` is provider-neutral and documented lowercase
/// (`["text"]`, `["audio"]`) — which is also what OpenAI's realtime API takes.
/// Gemini's `responseModalities` is a protobuf enum whose only valid JSON names
/// are `TEXT`, `AUDIO` and `IMAGE`, so the casing conversion belongs here at the
/// provider boundary rather than in the shared config.
///
/// This matters more than a cosmetic detail: sending lowercase `"audio"` was
/// accepted at setup (`setupComplete` came back) but the model then produced no
/// `serverContent` at all, so callers heard silence for the whole call.
/// Anything unrecognised is passed through uppercased rather than dropped, so a
/// new modality fails loudly at the API instead of silently degrading here.
fn gemini_response_modalities(modalities: Option<&[String]>) -> Vec<String> {
    match modalities {
        Some(values) if !values.is_empty() => values.iter().map(|m| m.to_uppercase()).collect(),
        // Audio-only is the live-call default for this crate.
        _ => vec!["AUDIO".to_string()],
    }
}

/// Per-connection identity and counter for frame telemetry.
///
/// Owned by the session rather than being a `static`, so two concurrent calls
/// produce two independently readable sequences instead of one interleaved
/// stream that cannot be untangled after the fact.
pub(crate) struct FrameLog {
    session_id: String,
    backend: &'static str,
    model: String,
    seq: std::sync::atomic::AtomicU64,
}

impl FrameLog {
    pub(crate) fn new(session_id: String, backend: &'static str, model: String) -> Self {
        Self { session_id, backend, model, seq: std::sync::atomic::AtomicU64::new(0) }
    }
}

/// Describes a frame that could not be parsed, without quoting it.
///
/// The previous form was `format!("Parse error: {e}, raw: {raw}")`, which put
/// the entire frame into an ordinary protocol error — and a Gemini frame carries
/// `inputTranscription.text`, base64 audio and tool arguments. That error string
/// travels wherever errors travel: logs, spans, an operator's terminal. The
/// shape logger directly below this exists precisely to avoid that, and the
/// error path went around it.
///
/// A length, a category and a hash are enough to correlate a bad frame with a
/// capture and to tell two different bad frames apart, which is what diagnosis
/// actually needs.
fn describe_unparseable_frame(raw: &str, error: &serde_json::Error) -> String {
    use std::hash::{Hash, Hasher};

    let category = match error.classify() {
        serde_json::error::Category::Io => "io",
        serde_json::error::Category::Syntax => "syntax",
        serde_json::error::Category::Data => "data",
        serde_json::error::Category::Eof => "eof",
    };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    raw.hash(&mut hasher);
    let payload_hash = hasher.finish();

    if cfg!(feature = "record-payloads") {
        return format!(
            "invalid Gemini frame (category={category}, byte_length={}, \
             payload_hash={payload_hash:016x}, line={}, column={}): {raw}",
            raw.len(),
            error.line(),
            error.column(),
        );
    }
    format!(
        "invalid Gemini frame (category={category}, byte_length={}, \
         payload_hash={payload_hash:016x}, line={}, column={})",
        raw.len(),
        error.line(),
        error.column(),
    )
}

/// Parse a human-readable or protobuf duration string (e.g. "10s", "120s", "500ms", "1m", "15.5s").
///
/// Safely returns `None` for negative, non-finite (NaN/infinity), overflowing, or malformed inputs without panicking.
fn parse_duration_string(s: &str) -> Option<std::time::Duration> {
    let s = s.trim();
    let secs = if let Some(stripped) = s.strip_suffix("ms") {
        let ms = stripped.trim().parse::<f64>().ok()?;
        ms / 1000.0
    } else if let Some(stripped) = s.strip_suffix('s') {
        stripped.trim().parse::<f64>().ok()?
    } else if let Some(stripped) = s.strip_suffix('m') {
        let m = stripped.trim().parse::<f64>().ok()?;
        m * 60.0
    } else {
        s.parse::<f64>().ok()?
    };

    if secs.is_finite() && (0.0..=86400.0 * 365.0).contains(&secs) {
        Some(std::time::Duration::from_secs_f64(secs))
    } else {
        None
    }
}

/// Frame-shape telemetry: what arrived, never what it said.
///
/// Deriving a caller-turn boundary on this provider needs the *ordering* of
/// lifecycle signals — Google states plainly that transcription "is sent
/// independently of the other server messages and there is no guaranteed
/// ordering". That ordering cannot be reasoned out from the type definitions;
/// it has to be observed on a real call.
///
/// The obvious way to observe it is to raise `translate_event_static`'s
/// existing `debug!(%raw, ..)` in production, and that is the wrong way: a raw
/// frame carries `inputTranscription.text` — the caller's own words — plus
/// base64 audio and tool arguments. This crate already treats that as
/// sensitive; it is why `record-payloads` exists and is off by default.
///
/// So this emits shape only, at `info`, and is safe to leave on: which message
/// arrived, the lifecycle booleans, how many parts and how many bytes, and the
/// *length* of a transcript rather than its text. That is sufficient to
/// reconstruct turn boundaries and still discloses nothing a caller said.
fn log_frame_shape(value: &Value, log: Option<&FrameLog>) {
    use std::sync::atomic::Ordering;

    // A process-global counter lived here, and the line carried no session
    // identity. With one call on the box a capture reads correctly; with two it
    // is a single interleaved sequence with no way to separate them, which is
    // the situation any real deployment is in. The number is only useful if it
    // is per-connection and the line says which connection.
    let (session_id, backend, model, seq) = match log {
        Some(log) => (
            log.session_id.as_str(),
            log.backend,
            log.model.as_str(),
            log.seq.fetch_add(1, Ordering::Relaxed),
        ),
        None => ("<detached>", "<detached>", "<detached>", 0),
    };
    let kind = [
        "setupComplete",
        "serverContent",
        "toolCall",
        "toolCallCancellation",
        "goAway",
        "sessionResumptionUpdate",
        "usageMetadata",
    ]
    .into_iter()
    .find(|k| value.get(*k).is_some())
    .unwrap_or("unknown");

    let content = value.get("serverContent");
    let flag =
        |name: &str| content.and_then(|c| c.get(name)).and_then(|v| v.as_bool()).unwrap_or(false);
    // Length, never the text.
    let text_len = |name: &str| {
        content
            .and_then(|c| c.get(name))
            .and_then(|t| t.get("text"))
            .and_then(|t| t.as_str())
            .map(str::len)
    };
    let parts = content
        .and_then(|c| c.get("modelTurn"))
        .and_then(|t| t.get("parts"))
        .and_then(|p| p.as_array());
    let audio_b64_len: usize = parts
        .map(|ps| {
            ps.iter().filter_map(|p| p.get("inlineData")?.get("data")?.as_str()).map(str::len).sum()
        })
        .unwrap_or(0);

    tracing::info!(
        target: "gemini_frame",
        session_id,
        backend,
        model,
        seq,
        kind,
        interrupted = flag("interrupted"),
        turn_complete = flag("turnComplete"),
        generation_complete = flag("generationComplete"),
        waiting_for_input = flag("waitingForInput"),
        interaction_status =
            content.and_then(|c| c.get("interactionStatus")).and_then(|v| v.as_str()),
        input_transcript_len = text_len("inputTranscription"),
        output_transcript_len = text_len("outputTranscription"),
        model_turn_parts = parts.map(|p| p.len()),
        audio_b64_len,
        tool_calls = value
            .get("toolCall")
            .and_then(|t| t.get("functionCalls"))
            .and_then(|c| c.as_array())
            .map(|c| c.len()),
        cancelled_tool_calls = value
            .get("toolCallCancellation")
            .and_then(|c| c.get("ids"))
            .and_then(|i| i.as_array())
            .map(|i| i.len()),
        "gemini frame"
    );
}

/// Project the crate's `VadConfig` onto Gemini's `realtimeInputConfig`.
///
/// Until this existed the setup message carried no `realtimeInputConfig` at
/// all, so every VAD setting a caller configured — including the
/// `.with_server_vad()` the data plane sets on every production session — was
/// accepted and silently discarded. A configuration API that implies control
/// it does not exert is worse than one that refuses: the caller believes the
/// line is tuned.
///
/// Returns `None` when there is nothing to say, so a default session keeps
/// sending no `realtimeInputConfig` and inherits Google's defaults.
///
/// Two fields deliberately do not map, and are warned about rather than
/// dropped in silence:
/// - `threshold` is a 0.0-1.0 probability; Gemini exposes only coarse
///   `START_SENSITIVITY_*` / `END_SENSITIVITY_*` enums, and inventing a
///   cutoff would be this adapter making up policy.
/// - `eagerness` is documented OpenAI-specific.
///
/// `activityHandling` is written only when the caller set
/// `interrupt_response`: `START_OF_ACTIVITY_INTERRUPTS` (the default) is the
/// behaviour [`ActivitySignaller::start`] depends on, so an explicit
/// `NO_INTERRUPTION` is a caller decision this projection honours rather than
/// silently dropping.
fn gemini_realtime_input_config(config: &RealtimeConfig) -> Option<GeminiRealtimeInputConfig> {
    let vad = config.turn_detection.as_ref()?;

    if vad.threshold.is_some() {
        tracing::warn!(
            "VadConfig.threshold is not representable on Gemini Live, which exposes only \
             coarse sensitivity levels; the configured value is not applied"
        );
    }
    if vad.eagerness.is_some() {
        tracing::debug!("VadConfig.eagerness is OpenAI-specific and does not apply to Gemini");
    }

    // `disabled` is never left implicit in an object we emit.
    //
    // It used to be written only for `VadMode::None`, so asking for one
    // tuning value under server VAD produced `{"silenceDurationMs": 1200}` —
    // an object whose central field, the one that decides who owns turn
    // detection at all, was absent. That exact payload reached production and
    // correlates with a pilot line that completed setup, held a healthy
    // socket, and returned no content at all. The vendor documents the partial
    // form as legal and the mechanism was never proven, so this is not a claim
    // to have found the bug — it is a refusal to keep sending a message whose
    // meaning depends on a field we chose not to write.
    //
    // The emptiness rule stays, and it is load-bearing: a caller who
    // configured nothing must still produce no `realtimeInputConfig` at all,
    // which is the wire shape server-side detection has always had. Only a
    // caller who said *something* gets an object, and then it is complete.
    // No rule is needed about which values to trust: `VadConfig`'s defaults
    // are `None`, so a `Some` here is a caller's decision by construction.
    let manual = vad.mode == crate::config::VadMode::None;
    let has_tuning = vad.silence_duration_ms.is_some()
        || vad.prefix_padding_ms.is_some()
        || vad.start_of_speech_sensitivity.is_some()
        || vad.end_of_speech_sensitivity.is_some();
    let automatic_activity_detection =
        (manual || has_tuning).then(|| GeminiAutomaticActivityDetection {
            disabled: manual,
            silence_duration_ms: vad.silence_duration_ms,
            prefix_padding_ms: vad.prefix_padding_ms,
            start_of_speech_sensitivity: vad.start_of_speech_sensitivity.map(|s| match s {
                crate::config::StartOfSpeechSensitivity::High => {
                    GeminiStartOfSpeechSensitivity::High
                }
                crate::config::StartOfSpeechSensitivity::Low => GeminiStartOfSpeechSensitivity::Low,
            }),
            end_of_speech_sensitivity: vad.end_of_speech_sensitivity.map(|s| match s {
                crate::config::EndOfSpeechSensitivity::High => GeminiEndOfSpeechSensitivity::High,
                crate::config::EndOfSpeechSensitivity::Low => GeminiEndOfSpeechSensitivity::Low,
            }),
        });
    let activity_handling = vad.interrupt_response.map(|interrupt| {
        if interrupt {
            GeminiActivityHandling::StartOfActivityInterrupts
        } else {
            GeminiActivityHandling::NoInterruption
        }
    });

    if automatic_activity_detection.is_none() && activity_handling.is_none() {
        return None;
    }
    Some(GeminiRealtimeInputConfig { automatic_activity_detection, activity_handling })
}

/// Construct the Vertex AI Live WebSocket URL from region and project ID.
///
/// Returns `RealtimeError::ConfigError` if region or project_id is empty.
#[cfg(feature = "vertex-live")]
pub fn build_vertex_live_url(region: &str, project_id: &str) -> Result<String> {
    if region.is_empty() {
        return Err(RealtimeError::config("Vertex AI Live requires a non-empty region"));
    }
    if project_id.is_empty() {
        return Err(RealtimeError::config("Vertex AI Live requires a non-empty project_id"));
    }
    Ok(format!(
        "wss://{region}-aiplatform.googleapis.com/ws/google.cloud.aiplatform.v1beta1.LlmBidiService/BidiGenerateContent?project_id={project_id}",
    ))
}

/// The `setup.model` resource name for `model` on `backend`.
///
/// AI Studio accepts only `models/{model}` and closes the socket with 1007 ("unexpected model
/// name format") on a bare id, so a bare id gains the prefix there. A name with a collection
/// (`models/…`, `tunedModels/…`) and every Vertex AI name pass unchanged.
pub(crate) fn live_model_name(backend: &GeminiLiveBackend, model: &str) -> String {
    match backend {
        GeminiLiveBackend::Studio { .. } if !model.contains('/') => format!("models/{model}"),
        _ => model.to_string(),
    }
}

/// The speech event a server message's `voiceActivity` reports, if it reports one.
///
/// The server's activity detection sends `voiceActivity {type, audioOffset}` at the user's
/// speech onset (`ACTIVITY_START`) and end (`ACTIVITY_END`), whether or not the model is
/// speaking, so it maps to [`ServerEvent::SpeechStarted`] / [`ServerEvent::SpeechStopped`]
/// as OpenAI's `input_audio_buffer.speech_started` / `speech_stopped` do. `audioOffset`, a
/// protobuf duration such as `"7.280s"`, is measured from the start of the audio stream.
fn voice_activity_event(value: &Value) -> Option<ServerEvent> {
    let activity = value.get("voiceActivity")?;
    let offset_ms = activity
        .get("audioOffset")
        .and_then(Value::as_str)
        .and_then(|offset| offset.strip_suffix('s')?.parse::<f64>().ok())
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .map_or(0, |seconds| (seconds * 1000.0).round() as u64);
    let event_id = uuid::Uuid::new_v4().to_string();
    match activity.get("type").and_then(Value::as_str)? {
        "ACTIVITY_START" => {
            Some(ServerEvent::SpeechStarted { event_id, audio_start_ms: offset_ms })
        }
        "ACTIVITY_END" => Some(ServerEvent::SpeechStopped { event_id, audio_end_ms: offset_ms }),
        _ => None,
    }
}

/// One camera or screen frame as `realtimeInput.video`.
fn video_frame_message<'a>(mime_type: &'a str, data_base64: &'a str) -> GeminiClientMessage<'a> {
    GeminiClientMessage {
        realtime_input: Some(GeminiRealtimeInput {
            video: Some(GeminiMediaChunk { mime_type, data: data_base64 }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Pure translation function for converting a standard `adk_core` message into
/// Gemini Live API's native `clientContent` payload.
pub(crate) fn translate_client_message(
    role: &str,
    parts: Vec<adk_core::types::Part>,
) -> GeminiClientMessage<'_> {
    // 1. Translate the polymorphic `adk_core::types::Part` elements into strictly-typed `GeminiPart` structures.
    let mut gemini_parts: Vec<GeminiPart> = Vec::new();
    for p in parts {
        match p {
            adk_core::types::Part::Text { text } => {
                gemini_parts.push(GeminiPart { text: Some(text), inline_data: None });
            }
            adk_core::types::Part::InlineData { mime_type, data, .. } => {
                // Gemini natively encodes binary artifacts (images/audio) via a base64 payload envelope.
                let encoded = base64::engine::general_purpose::STANDARD.encode(&data);
                gemini_parts.push(GeminiPart {
                    text: None,
                    inline_data: Some(GeminiInlineData { mime_type, data: encoded }),
                });
            }

            // 2. Gracefully skip semantic elements that Google's Live API `clientContent` turn does not support
            // using `tracing::warn!`, avoiding "silent data loss" or injecting empty string placeholders.
            adk_core::types::Part::FileData { file_uri, .. } => {
                tracing::warn!(
                    "Dropping unsupported FileData part in Gemini session: {}",
                    file_uri
                );
            }
            adk_core::types::Part::Thinking { .. } => {
                tracing::warn!("Dropping unsupported Thinking part in Gemini session");
            }
            adk_core::types::Part::FunctionCall { name, .. } => {
                tracing::warn!(
                    "Dropping unsupported FunctionCall part in Gemini session: {}",
                    name
                );
            }
            adk_core::types::Part::FunctionResponse { .. } => {
                tracing::warn!("Dropping unsupported FunctionResponse part in Gemini session");
            }
            adk_core::types::Part::ServerToolCall { .. } => {
                tracing::warn!("Dropping unsupported ServerToolCall part in Gemini session");
            }
            adk_core::types::Part::ServerToolResponse { .. } => {
                tracing::warn!("Dropping unsupported ServerToolResponse part in Gemini session");
            }
            adk_core::types::Part::EmbeddedResource { resource } => {
                tracing::warn!(
                    "Dropping unsupported EmbeddedResource part in Gemini session: {}",
                    resource.uri()
                );
            }
        }
    }

    // 3. Coerce the `Role`.
    // Gemini Live's bidirectional socket strongly rejects `system` or `developer` roles
    // inside mid-flight `clientContent` turns. To support Cognitive Handoffs, we intercept
    // the system instruction and safely masquerade it as a high-priority "user" turn.
    let (gemini_role, final_parts) = match role {
        "system" | "developer" => {
            let mut modified_parts = gemini_parts;
            let mut text_injected = false;

            // Iterate to find the first actual text element in the user's prompt (avoiding images/audio arrays)
            // to safely inject the system prefix.
            for part in modified_parts.iter_mut() {
                if let Some(ref mut text) = part.text {
                    *text = format!("[CRITICAL SYSTEM DIRECTIVE OVERRIDE]\n{}", text);
                    text_injected = true;
                    break;
                }
            }

            // If the orchestrator sent a `system` role containing exclusively non-text data (e.g., just an image),
            // construct a synthetic text part to carry the directive.
            if !text_injected {
                modified_parts.insert(
                    0,
                    GeminiPart {
                        text: Some("[CRITICAL SYSTEM DIRECTIVE OVERRIDE]".to_string()),
                        inline_data: None,
                    },
                );
            }

            ("user".to_string(), modified_parts)
        }
        "user" => ("user".to_string(), gemini_parts),
        "model" | "assistant" => ("model".to_string(), gemini_parts),
        _ => ("user".to_string(), gemini_parts), // Default fallback for custom orchestrator roles
    };

    // 4. Construct the native `GeminiClientContent` wire envelope.
    GeminiClientMessage {
        client_content: Some(GeminiClientContent {
            turns: vec![GeminiTurn { role: gemini_role, parts: final_parts }],
            turn_complete: true,
        }),
        ..Default::default()
    }
}

/// Convert ADK tool definitions to Gemini format.
///
/// `dialect` decides the field the schema is posted under. It is a parameter
/// rather than a constant because Gemini's two schema fields are mutually
/// exclusive and express different things: `parameters` takes an OpenAPI
/// subset, `parametersJsonSchema` takes standard JSON Schema. Hardcoding the
/// legacy field silently discards every constraint the subset cannot carry —
/// `additionalProperties`, `allOf`, `if`/`then`, the exclusive bounds — leaving
/// the model to guess at rules the caller still enforces.
fn convert_tools(
    tools: Option<Vec<ToolDefinition>>,
    cache: &adk_core::SchemaCache,
    adapter: &dyn adk_core::SchemaAdapter,
    caps: GeminiLiveCapabilities,
) -> Result<Option<Vec<Value>>> {
    let Some(tools) = tools else {
        return Ok(None);
    };

    let mut declarations = Vec::new();
    for t in tools {
        adapter
            .validate_tool_name(&t.name)
            .map_err(|e| RealtimeError::protocol(format!("Invalid tool name {}: {}", t.name, e)))?;

        let mut decl = json!({ "name": t.name });
        if caps.async_functions {
            decl["behavior"] = json!("BLOCKING");
        }
        if let Some(desc) = &t.description {
            decl["description"] = json!(desc);
        }
        // `behavior` is a Gemini Live declaration field with no OpenAI
        // counterpart; the OpenAI session setup never reads it.
        if let Some(behavior) = &t.behavior {
            decl["behavior"] = serde_json::to_value(behavior).map_err(|e| {
                RealtimeError::protocol(format!(
                    "Failed to serialize behavior for tool {}: {e}",
                    t.name
                ))
            })?;
        }
        // The adapter names the field, because the field and the dialect are one
        // decision. `parameters` and `parametersJsonSchema` are mutually
        // exclusive, and posting a schema under the wrong one is not a degraded
        // request — a keyword the OpenAPI subset does not know closes the Live
        // socket with WS 1007 during setup, before the first turn. Writing
        // exactly one key makes the exclusivity structural.
        let field = adapter.parameters_field();
        if let Some(params) = &t.parameters {
            let compiled = cache.get_or_compile(params, adapter).map_err(|e| {
                RealtimeError::protocol(format!(
                    "Failed to compile schema for tool {}: {}",
                    t.name, e
                ))
            })?;
            decl[field] = compiled;
        } else {
            decl[field] = adapter.empty_schema();
        }
        declarations.push(decl);
    }

    Ok(Some(vec![json!({
        "functionDeclarations": declarations
    })]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use adk_core::types::Part;
    use tokio_tungstenite::tungstenite::protocol::CloseFrame;
    use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

    mod tool_declarations {
        use super::*;
        use crate::config::ToolBehavior;

        fn intake_tool() -> ToolDefinition {
            ToolDefinition::new("submit_conversational_result").with_parameters(json!({
                "type": "object",
                "properties": {
                    "callback_number": { "type": "string", "minLength": 7 }
                },
                "required": ["callback_number"],
                "additionalProperties": false
            }))
        }

        fn declaration(dialect: GeminiSchemaDialect) -> Value {
            let adapter = adk_gemini::schema_adapter::GeminiSchemaAdapter::with_dialect(dialect);
            let cache = adk_core::SchemaCache::new();
            let caps = capabilities_for("models/gemini-3.1-flash-live-preview");
            let tools = convert_tools(Some(vec![intake_tool()]), &cache, &adapter, caps)
                .expect("the fixture compiles")
                .expect("tools were supplied");
            tools[0]["functionDeclarations"][0].clone()
        }

        fn declaration_for(tool: ToolDefinition, model: &str) -> Value {
            let adapter = adk_gemini::schema_adapter::GeminiSchemaAdapter::json_schema();
            let cache = adk_core::SchemaCache::new();
            let caps = capabilities_for(model);
            let tools = convert_tools(Some(vec![tool]), &cache, &adapter, caps)
                .expect("the fixture compiles")
                .expect("tools were supplied");
            tools[0]["functionDeclarations"][0].clone()
        }

        /// The field is chosen by the dialect, and exactly one is written.
        /// `parameters` and `parametersJsonSchema` are mutually exclusive, and
        /// sending a JSON Schema under the older name closes the Live socket
        /// with WS 1007 during setup — a dead call, not a degraded one.
        #[test]
        fn each_dialect_writes_its_own_field_and_only_its_own() {
            let subset = declaration(GeminiSchemaDialect::OpenApiSubset);
            assert!(subset.get("parameters").is_some(), "{subset}");
            assert!(subset.get("parametersJsonSchema").is_none(), "{subset}");

            let json_schema = declaration(GeminiSchemaDialect::JsonSchema);
            assert!(json_schema.get("parametersJsonSchema").is_some(), "{json_schema}");
            assert!(json_schema.get("parameters").is_none(), "{json_schema}");
        }

        /// The reason the field matters: the constraints the caller is judged
        /// against reach the model on one dialect and not the other.
        #[test]
        fn the_json_schema_field_carries_constraints_the_older_one_drops() {
            let subset = declaration(GeminiSchemaDialect::OpenApiSubset);
            // The subset keeps string/numeric constraints such as `minLength`;
            // what it cannot carry is `additionalProperties` (and composition).
            assert_eq!(subset["parameters"]["properties"]["callback_number"]["minLength"], 7);
            assert!(subset["parameters"].get("additionalProperties").is_none());

            let json_schema = declaration(GeminiSchemaDialect::JsonSchema);
            assert_eq!(
                json_schema["parametersJsonSchema"]["properties"]["callback_number"]["minLength"],
                7
            );
            assert_eq!(json_schema["parametersJsonSchema"]["additionalProperties"], json!(false));
        }

        /// A tool with no parameters still declares an empty object, under the
        /// same field — otherwise the fallback quietly reintroduces the split.
        #[test]
        fn a_parameterless_tool_declares_its_empty_schema_on_the_same_field() {
            let adapter = adk_gemini::schema_adapter::GeminiSchemaAdapter::json_schema();
            let cache = adk_core::SchemaCache::new();
            let caps = capabilities_for("models/gemini-3.1-flash-live-preview");

            let tools =
                convert_tools(Some(vec![ToolDefinition::new("hangup")]), &cache, &adapter, caps)
                    .expect("a parameterless tool compiles")
                    .expect("tools were supplied");
            let declaration = &tools[0]["functionDeclarations"][0];

            assert_eq!(
                declaration["parametersJsonSchema"],
                json!({"type": "object", "properties": {}})
            );
            assert!(declaration.get("parameters").is_none(), "{declaration}");
        }

        /// A declared behavior overrides the capability default, so a
        /// NON_BLOCKING tool actually runs async on 3.8 — otherwise the
        /// declaration is a comment and the turn blocks while the caller pays.
        #[test]
        fn a_declared_behavior_overrides_the_capability_default() {
            let declaration = declaration_for(
                ToolDefinition::new("request_payment").with_behavior(ToolBehavior::NonBlocking),
                "models/gemini-3.8-live",
            );

            assert_eq!(declaration["behavior"], json!("NON_BLOCKING"), "{declaration}");
        }

        /// No declared behavior means the capability default decides: 3.8
        /// declares BLOCKING, older models send no key and keep their old wire.
        #[test]
        fn unset_behavior_falls_back_to_the_capability_default() {
            let modern = declaration_for(ToolDefinition::new("hangup"), "models/gemini-3.8-live");
            assert_eq!(modern["behavior"], json!("BLOCKING"), "{modern}");

            let legacy = declaration_for(
                ToolDefinition::new("hangup"),
                "models/gemini-3.1-flash-live-preview",
            );
            assert!(legacy.get("behavior").is_none(), "{legacy}");
        }

        /// Studio keeps its historical dialect unless a caller opts in.
        /// `parametersJsonSchema` is absent from Google's reference docs and
        /// verified only by probe, which is not a basis for changing what every
        /// consumer of this crate puts on the wire.
        #[test]
        fn the_default_dialect_is_unchanged() {
            let backend = GeminiLiveBackend::studio("unused-in-this-test");

            assert_eq!(
                GeminiRealtimeSession::default_schema_dialect(&backend),
                GeminiSchemaDialect::OpenApiSubset
            );
        }
    }

    #[test]
    fn test_gemini_send_text_uses_realtime_input() {
        // Conversational text turns must serialize via realtimeInput.text for Gemini 3.8 Live
        let msg = GeminiClientMessage {
            realtime_input: Some(GeminiRealtimeInput {
                text: Some("Hello 3.8".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let js = serde_json::to_value(&msg).unwrap();
        assert!(js.get("clientContent").is_none());
        assert_eq!(
            js.get("realtimeInput").and_then(|r| r.get("text")).and_then(|t| t.as_str()),
            Some("Hello 3.8")
        );
    }

    #[test]
    fn test_gemini_translate_text_only() {
        let parts = vec![Part::Text { text: "Hello".to_string() }];
        let msg = translate_client_message("user", parts);

        let content = msg.client_content.unwrap();
        assert_eq!(content.turns.len(), 1);
        assert_eq!(content.turns[0].role, "user");

        let gemini_parts = &content.turns[0].parts;
        assert_eq!(gemini_parts.len(), 1);
        assert_eq!(gemini_parts[0].text.as_deref(), Some("Hello"));
        assert!(gemini_parts[0].inline_data.is_none());
    }

    #[test]
    fn test_gemini_translate_text_and_inline_data() {
        let parts = vec![
            Part::Text { text: "Look:".to_string() },
            Part::inline_data("image/png", vec![0x1, 0x2]),
        ];
        let msg = translate_client_message("user", parts);

        let content = msg.client_content.unwrap();
        let gemini_parts = &content.turns[0].parts;
        assert_eq!(gemini_parts.len(), 2);

        assert_eq!(gemini_parts[0].text.as_deref(), Some("Look:"));

        let inline = gemini_parts[1].inline_data.as_ref().unwrap();
        assert_eq!(inline.mime_type, "image/png");
        assert_eq!(inline.data, "AQI="); // base64 encoded [1,2]
    }

    #[test]
    fn test_gemini_system_override_text_first() {
        let parts = vec![Part::Text { text: "Be helpful".to_string() }];
        let msg = translate_client_message("system", parts);

        let content = msg.client_content.unwrap();
        assert_eq!(content.turns[0].role, "user"); // coerced

        let gemini_parts = &content.turns[0].parts;
        assert_eq!(gemini_parts.len(), 1);
        assert_eq!(
            gemini_parts[0].text.as_deref(),
            Some("[CRITICAL SYSTEM DIRECTIVE OVERRIDE]\nBe helpful")
        );
    }

    /// A malformed frame must not put what the caller said into an error string.
    ///
    /// The previous form interpolated the whole frame, and a Gemini frame
    /// carries `inputTranscription.text`, base64 audio and tool arguments.
    #[test]
    fn unparseable_frames_do_not_leak_their_contents() {
        let raw = r#"{"serverContent":{"inputTranscription":{"text":"my card number is 4111"#;
        let error = GeminiRealtimeSession::translate_event_static(raw)
            .expect_err("a truncated frame must not parse");
        let rendered = error.to_string();

        assert!(
            !rendered.contains("4111") && !rendered.contains("my card number"),
            "caller content reached an error string: {rendered}"
        );
        assert!(rendered.contains("byte_length="), "no length to diagnose with: {rendered}");
        assert!(rendered.contains("payload_hash="), "no hash to correlate with: {rendered}");
        assert!(rendered.contains("category="), "no parse category: {rendered}");
    }

    /// Two different bad frames must be distinguishable, or the hash is useless
    /// for telling a repeated fault from a new one.
    #[test]
    fn unparseable_frames_hash_differently() {
        let first = GeminiRealtimeSession::translate_event_static(r#"{"a":"#)
            .expect_err("truncated")
            .to_string();
        let second = GeminiRealtimeSession::translate_event_static(r#"{"b":"#)
            .expect_err("truncated")
            .to_string();
        assert_ne!(first, second);
    }

    /// Resumable sessionResumptionUpdate control frame produces no public ServerEvent.
    #[test]
    fn resumable_session_update_yields_no_public_event() {
        let raw = r#"{"sessionResumptionUpdate":{"newHandle":"handle-abc","resumable":true}}"#;
        let events = GeminiRealtimeSession::translate_event_static(raw).unwrap();
        assert!(
            events.is_empty(),
            "expected no public events for resumable update, got {events:?}"
        );
    }

    /// `resumable: false` means "no safe resume point right now". Emitting the
    /// handle anyway would let a caller overwrite a working resume point with an
    /// unusable one.
    #[test]
    fn non_resumable_session_update_yields_no_handle() {
        for raw in [
            r#"{"sessionResumptionUpdate":{"newHandle":"","resumable":true}}"#,
            r#"{"sessionResumptionUpdate":{"newHandle":"   ","resumable":true}}"#,
            r#"{"sessionResumptionUpdate":{"newHandle":"handle-abc","resumable":false}}"#,
            r#"{"sessionResumptionUpdate":{"newHandle":"handle-abc"}}"#,
            r#"{"sessionResumptionUpdate":{"resumable":true}}"#,
            // The field the old code read. It is not part of the contract.
            r#"{"sessionResumptionUpdate":{"resumptionToken":"handle-abc"}}"#,
        ] {
            let events = GeminiRealtimeSession::translate_event_static(raw).unwrap();
            assert!(events.is_empty(), "expected no event for {raw}, got {events:?}");
        }
    }

    #[test]
    fn test_gemini_system_override_non_text_first() {
        let parts = vec![
            Part::inline_data("image/png", vec![0x1]),
            Part::Text { text: "Analyze this".to_string() },
        ];
        let msg = translate_client_message("system", parts);

        let content = msg.client_content.unwrap();
        let gemini_parts = &content.turns[0].parts;
        assert_eq!(gemini_parts.len(), 2);

        // Ensure the directive was applied to the FIRST text part, despite being index 1
        assert!(gemini_parts[0].inline_data.is_some());
        assert_eq!(
            gemini_parts[1].text.as_deref(),
            Some("[CRITICAL SYSTEM DIRECTIVE OVERRIDE]\nAnalyze this")
        );
    }

    #[test]
    fn test_gemini_system_override_no_text() {
        let parts = vec![Part::inline_data("image/png", vec![0x1])];
        let msg = translate_client_message("system", parts);

        let content = msg.client_content.unwrap();
        let gemini_parts = &content.turns[0].parts;
        assert_eq!(gemini_parts.len(), 2);

        // Ensure a new text part was inserted at the beginning
        assert_eq!(gemini_parts[0].text.as_deref(), Some("[CRITICAL SYSTEM DIRECTIVE OVERRIDE]"));
        assert!(gemini_parts[1].inline_data.is_some());
    }

    #[test]
    fn test_gemini_skips_unsupported_parts() {
        let parts = vec![
            Part::Text { text: "First".to_string() },
            Part::FileData {
                mime_type: "image/jpeg".to_string(),
                file_uri: "http://example.com/img".to_string(),
                annotations: None,
            }, // Should be skipped
            Part::Thinking { thinking: "Hmm".to_string(), signature: None }, // Should be skipped
            Part::Text { text: "Last".to_string() },
        ];
        let msg = translate_client_message("user", parts);

        let content = msg.client_content.unwrap();
        let gemini_parts = &content.turns[0].parts;
        assert_eq!(gemini_parts.len(), 2);

        assert_eq!(gemini_parts[0].text.as_deref(), Some("First"));
        assert_eq!(gemini_parts[1].text.as_deref(), Some("Last"));
    }
    #[test]
    fn test_translate_gemini_event_strict_validation() {
        // 1. Missing name
        let raw = json!({
            "toolCall": {
                "functionCalls": [{
                    "id": "call_1",
                    "args": {}
                }]
            }
        })
        .to_string();
        let result = GeminiRealtimeSession::translate_event_static(&raw);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Gemini tool call missing 'name'"));

        // 2. Missing id
        let raw = json!({
            "toolCall": {
                "functionCalls": [{
                    "name": "test",
                    "args": {}
                }]
            }
        })
        .to_string();
        let result = GeminiRealtimeSession::translate_event_static(&raw);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Gemini tool call missing 'id'"));

        // 3. Missing args
        let raw = json!({
            "toolCall": {
                "functionCalls": [{
                    "name": "test",
                    "id": "call_1"
                }]
            }
        })
        .to_string();
        let result = GeminiRealtimeSession::translate_event_static(&raw);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Gemini tool call missing 'args'"));

        // 4. Args not an object
        let raw = json!({
            "toolCall": {
                "functionCalls": [{
                    "name": "test",
                    "id": "call_1",
                    "args": "not_an_object"
                }]
            }
        })
        .to_string();
        let result = GeminiRealtimeSession::translate_event_static(&raw);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("Gemini tool call 'args' must be an object")
        );

        // 5. Valid call
        let raw = json!({
            "toolCall": {
                "functionCalls": [{
                    "name": "test",
                    "id": "call_1",
                    "args": {"key": "value"}
                }]
            }
        })
        .to_string();
        let events = GeminiRealtimeSession::translate_event_static(&raw).unwrap();
        assert_eq!(events.len(), 1);
        if let ServerEvent::FunctionCallDone { name, call_id, arguments, .. } = &events[0] {
            assert_eq!(name, "test");
            assert_eq!(call_id, "call_1");
            assert_eq!(arguments, &json!({"key": "value"}));
        } else {
            panic!("Expected FunctionCallDone");
        }
    }

    #[test]
    fn test_gemini_setup_serialization_includes_model() {
        let setup = GeminiSetup {
            model: Some("models/gemini-2.5-flash-native-audio-latest".to_string()),
            system_instruction: None,
            generation_config: None,
            tools: None,
            cached_content: None,
            session_resumption: None,
            input_audio_transcription: None,
            output_audio_transcription: None,
            realtime_input_config: None,
            context_window_compression: None,
            proactivity: None,
        };
        let wrapper = GeminiClientMessage { setup: Some(setup), ..Default::default() };
        let js = serde_json::to_value(&wrapper).unwrap();
        let setup_json = js.get("setup").expect("setup missing").as_object().unwrap();
        assert_eq!(
            setup_json.get("model").expect("model missing from setup payload").as_str().unwrap(),
            "models/gemini-2.5-flash-native-audio-latest"
        );
    }

    #[test]
    fn test_language_code_lands_in_speech_config_next_to_the_voice() {
        let js = setup_json(
            RealtimeConfig::default().with_voice("Aoede").try_with_language("en-US").unwrap(),
        );
        let speech = &js["setup"]["generationConfig"]["speechConfig"];
        assert_eq!(speech["languageCode"], json!("en-US"));
        assert_eq!(speech["voiceConfig"]["prebuiltVoiceConfig"]["voiceName"], json!("Aoede"));
    }

    #[test]
    fn test_language_code_without_a_voice_still_builds_speech_config() {
        let js = setup_json(RealtimeConfig::default().try_with_language("en-GB").unwrap());
        assert_eq!(js["setup"]["generationConfig"]["speechConfig"]["languageCode"], json!("en-GB"));
        assert!(
            setup_json(RealtimeConfig::default())["setup"]["generationConfig"]
                .get("speechConfig")
                .is_none()
        );
        // An invalid tag is rejected before it can reach a setup.
        assert!(RealtimeConfig::default().try_with_language("").is_err());
    }

    #[test]
    fn test_language_code_lives_in_the_typed_field_not_extra() {
        let config =
            RealtimeConfig { extra: Some(json!({"thinking_level": "low"})), ..Default::default() };
        let config = config.try_with_language("en-US").unwrap();
        assert_eq!(config.language.as_ref().map(|code| code.as_str()), Some("en-US"));
        assert_eq!(config.extra, Some(json!({"thinking_level": "low"})));

        let config =
            RealtimeConfig { extra: Some(json!(["not", "an", "object"])), ..Default::default() };
        let config = config.try_with_language("fr-FR").unwrap();
        assert_eq!(config.language.as_ref().map(|code| code.as_str()), Some("fr-FR"));
        assert_eq!(config.extra, Some(json!(["not", "an", "object"])));
    }

    #[test]
    fn test_session_resumption_is_on_by_default_and_off_when_disabled() {
        let setup = setup_json(RealtimeConfig::default());
        assert!(setup["setup"].get("sessionResumption").is_some(), "{setup}");
        let setup = setup_json(RealtimeConfig::default().with_session_resumption(false));
        assert!(setup["setup"].get("sessionResumption").is_none(), "{setup}");
    }

    #[test]
    fn test_context_window_compression_sends_a_sliding_window() {
        let js = setup_json(RealtimeConfig::default().with_context_window_compression(None));
        assert_eq!(js["setup"]["contextWindowCompression"], json!({"slidingWindow": {}}));

        let js = setup_json(
            RealtimeConfig::default()
                .try_with_language("en-US")
                .unwrap()
                .with_context_window_compression(Some(25_600)),
        );
        assert_eq!(
            js["setup"]["contextWindowCompression"],
            json!({"slidingWindow": {}, "triggerTokens": 25_600})
        );
        assert_eq!(js["setup"]["generationConfig"]["speechConfig"]["languageCode"], json!("en-US"));

        let config = RealtimeConfig {
            extra: Some(json!({
                "context_window_compression": {"trigger_tokens": 32_000, "target_tokens": 16_000}
            })),
            ..Default::default()
        };
        assert_eq!(
            setup_json(config)["setup"]["contextWindowCompression"],
            json!({"slidingWindow": {"targetTokens": 16_000}, "triggerTokens": 32_000})
        );
    }

    #[test]
    fn test_context_window_compression_is_absent_unless_asked() {
        assert!(
            setup_json(RealtimeConfig::default())["setup"]
                .get("contextWindowCompression")
                .is_none()
        );
        for off in [json!(false), json!(null), json!("yes"), json!(1)] {
            let config = RealtimeConfig {
                extra: Some(json!({"context_window_compression": off})),
                ..Default::default()
            };
            assert!(
                setup_json(config)["setup"].get("contextWindowCompression").is_none(),
                "{off} must not turn compression on"
            );
        }
    }

    #[test]
    fn test_affective_dialog_builder_sets_config() {
        let c = RealtimeConfig::default().with_affective_dialog(true);
        assert_eq!(c.affective_dialog, Some(true));
        assert_eq!(RealtimeConfig::default().affective_dialog, None);
    }

    #[test]
    fn test_proactive_audio_builder_sets_config() {
        let c = RealtimeConfig::default().with_proactive_audio(true);
        assert_eq!(c.proactive_audio, Some(true));
        assert_eq!(RealtimeConfig::default().proactive_audio, None);
    }

    #[test]
    fn test_flush_threshold_bytes_pcm16_16khz_40ms() {
        let threshold = GeminiRealtimeSession::flush_threshold_bytes(&AudioFormat::pcm16_16khz());
        assert_eq!(threshold, 1280);
    }

    #[cfg(feature = "vertex-live")]
    #[test]
    fn test_build_vertex_live_url() {
        let url = build_vertex_live_url("us-central1", "my-project").unwrap();
        assert_eq!(
            url,
            "wss://us-central1-aiplatform.googleapis.com/ws/google.cloud.aiplatform.v1beta1.LlmBidiService/BidiGenerateContent?project_id=my-project"
        );
    }

    #[tokio::test]
    async fn test_connect_fail_closed_on_invalid_tool_name() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let backend = GeminiLiveBackend::studio("fake-key");
        let config = RealtimeConfig {
            tools: Some(vec![ToolDefinition {
                name: "a".repeat(65),
                description: Some("description".to_string()),
                parameters: Some(json!({"type": "object"})),
                behavior: None,
                effect: adk_core::ToolEffect::NonIdempotent,
            }]),
            ..Default::default()
        };

        let result = GeminiRealtimeSession::connect(backend, "models/gemini-live", config).await;

        match result {
            Err(RealtimeError::Protocol(msg)) => {
                assert!(msg.contains("Invalid tool name"));
                assert!(msg.contains("64-character limit"));
            }
            other => panic!(
                "Expected Protocol Error (via RealtimeError::protocol) due to tool name validation failure, got {:?}",
                other
            ),
        }
    }

    #[test]
    fn server_vad_defaults_do_not_reach_the_wire() {
        // `.with_server_vad()` is `VadConfig::default()`, so every field in it
        // is a library default rather than a choice. The data plane sets this
        // on every production session; forwarding it would install a 500 ms
        // end-of-speech threshold — the *minimum* Google recommends, against
        // its own ~800 ms server default — as live turn-taking policy on a
        // phone line, purely because a struct had to initialise somehow.
        assert!(
            gemini_realtime_input_config(&RealtimeConfig::default().with_server_vad()).is_none(),
            "asking for server VAD must not silently pin OpenAI-shaped timings"
        );
    }

    /// **Protocol for settling which of these shapes is safe** (not run here —
    /// it needs a live key and real speech, and this crate has neither).
    ///
    /// The pilot line went silent for hours after a partial
    /// `automaticActivityDetection` first reached the wire, and the mechanism
    /// was never proven: the correlation is confounded (the same deploy moved
    /// the contract revision, dropped two tools and grew the system
    /// instruction by ~9 KB), and the vendor documents the partial form as
    /// legal. Until someone runs this, "the partial payload silenced the
    /// model" is a hypothesis, and restoring an end-of-turn budget rests on it.
    ///
    /// Three sessions, same speech audio, count `serverContent` in each:
    ///
    /// 1. no `realtimeInputConfig` at all — the known-good shape, and what
    ///    production sends today;
    /// 2. `{"silenceDurationMs": 1200, "disabled": false}` — what this
    ///    projection now produces, and what a restored budget would send;
    /// 3. `{"silenceDurationMs": 1200}` — the suspect, which this projection
    ///    can no longer produce and which therefore has to be handcrafted.
    ///
    /// **All three must travel the same path.** Sending 1 and 2 through
    /// `GeminiRealtimeSession` and handcrafting 3 over a raw socket would
    /// confound the payload with the plumbing: a difference in outcome could
    /// then be either the bytes or the client, and the experiment would prove
    /// nothing it set out to prove. Send all three as raw setup frames.
    ///
    /// It also cannot live in this crate: it needs a real speech recording to
    /// get any `serverContent` at all, a synthesised tone will not do, and the
    /// fixtures are in `zenith/data_plane/testdata/voice`. Run it from there.
    ///
    /// What each outcome licenses:
    ///
    /// - 3 silent, 1 and 2 fine → the partial payload is the mechanism, the
    ///   fix above is the fix, and the budget can be restored.
    /// - all three fine → the mechanism is elsewhere; the silence had another
    ///   cause and this projection change is merely hygiene. Restoring the
    ///   budget is then safe but the outage is still unexplained, which is
    ///   worth saying out loud rather than quietly closing.
    /// - 2 silent → do **not** restore the budget in any form, and the
    ///   complete-payload theory is wrong too.
    ///
    /// The whole object, pinned.
    ///
    /// A partial `automaticActivityDetection` reached production once — one
    /// tuning value with no `disabled` beside it — and nothing in the
    /// repository caught it: every test asserted the fields it cared about and
    /// none asserted the shape. Field-by-field assertions cannot fail for an
    /// *absent* field nobody thought to name, which is exactly the failure
    /// mode. This compares the serialized object whole, so any future omission
    /// or addition has to be looked at by a human before it ships.
    #[test]
    fn the_projected_detection_object_is_pinned_whole() {
        use crate::config::{EndOfSpeechSensitivity, StartOfSpeechSensitivity, VadConfig, VadMode};

        // One tuning value under server VAD: the shape that went to production.
        let one_value = RealtimeConfig::default()
            .with_vad(VadConfig { silence_duration_ms: Some(1200), ..VadConfig::server_vad() });
        assert_eq!(
            serde_json::to_value(gemini_realtime_input_config(&one_value).expect("projection"))
                .unwrap()["automaticActivityDetection"],
            json!({ "silenceDurationMs": 1200, "disabled": false }),
            "a tuning value must not travel without saying who owns detection"
        );

        // Everything set.
        let full = RealtimeConfig::default().with_vad(VadConfig {
            silence_duration_ms: Some(1200),
            prefix_padding_ms: Some(300),
            start_of_speech_sensitivity: Some(StartOfSpeechSensitivity::Low),
            end_of_speech_sensitivity: Some(EndOfSpeechSensitivity::Low),
            ..VadConfig::server_vad()
        });
        assert_eq!(
            serde_json::to_value(gemini_realtime_input_config(&full).expect("projection")).unwrap()
                ["automaticActivityDetection"],
            json!({
                "silenceDurationMs": 1200,
                "prefixPaddingMs": 300,
                "startOfSpeechSensitivity": "START_SENSITIVITY_LOW",
                "endOfSpeechSensitivity": "END_SENSITIVITY_LOW",
                "disabled": false
            })
        );

        // Manual mode with no tuning: `disabled` alone, and true.
        let manual = RealtimeConfig::default()
            .with_vad(VadConfig { mode: VadMode::None, ..VadConfig::server_vad() });
        assert_eq!(
            serde_json::to_value(gemini_realtime_input_config(&manual).expect("projection"))
                .unwrap()["automaticActivityDetection"],
            json!({ "disabled": true })
        );

        // Nothing configured: no object at all, which is the wire shape
        // server-side detection has always had. The emptiness check that
        // preserves this is the reason `disabled` is not simply always set.
        assert!(
            gemini_realtime_input_config(&RealtimeConfig::default().with_server_vad()).is_none(),
            "a caller who chose nothing must send no realtimeInputConfig"
        );
    }

    #[test]
    fn explicitly_chosen_timings_do_reach_the_wire() {
        use crate::config::{VadConfig, VadMode};

        let config = RealtimeConfig::default().with_vad(VadConfig {
            mode: VadMode::ServerVad,
            silence_duration_ms: Some(900),
            prefix_padding_ms: Some(300),
            threshold: None,
            interrupt_response: None,
            eagerness: None,
            start_of_speech_sensitivity: None,
            end_of_speech_sensitivity: None,
        });
        let projected =
            serde_json::to_value(gemini_realtime_input_config(&config).expect("projection"))
                .unwrap();
        let detection = projected.get("automaticActivityDetection").expect("detection");

        assert_eq!(detection.get("silenceDurationMs").and_then(|v| v.as_u64()), Some(900));
        assert_eq!(detection.get("prefixPaddingMs").and_then(|v| v.as_u64()), Some(300));
        assert!(
            projected.get("activityHandling").is_none(),
            "an unset interrupt policy must not be invented"
        );
    }

    #[test]
    fn vad_timings_and_disable_map_to_googles_names() {
        use crate::config::{VadConfig, VadMode};

        let config = RealtimeConfig::default().with_vad(VadConfig {
            mode: VadMode::None,
            silence_duration_ms: Some(700),
            prefix_padding_ms: Some(300),
            threshold: None,
            interrupt_response: Some(false),
            eagerness: None,
            start_of_speech_sensitivity: None,
            end_of_speech_sensitivity: None,
        });
        let projected =
            serde_json::to_value(gemini_realtime_input_config(&config).expect("projection"))
                .unwrap();
        let detection = projected.get("automaticActivityDetection").expect("detection block");

        assert_eq!(detection.get("disabled").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(detection.get("silenceDurationMs").and_then(|v| v.as_u64()), Some(700));
        assert_eq!(detection.get("prefixPaddingMs").and_then(|v| v.as_u64()), Some(300));
        assert_eq!(
            projected.get("activityHandling").and_then(|v| v.as_str()),
            Some("NO_INTERRUPTION")
        );
    }

    // ── Activity signalling ─────────────────────────────────────────────

    fn setup_json(config: RealtimeConfig) -> Value {
        let msg = GeminiRealtimeSession::build_setup_message("models/test", config, None, None)
            .expect("setup builds");
        serde_json::to_value(&msg).expect("setup serializes")
    }

    #[test]
    fn activity_start_serializes_as_documented() {
        let msg = GeminiClientMessage {
            realtime_input: Some(ActivitySignal::Start.into_realtime_input()),
            ..Default::default()
        };

        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            json!({ "realtimeInput": { "activityStart": {} } })
        );
    }
    #[test]
    fn no_vad_configured_sends_no_input_config() {
        // A default session must keep inheriting Google's defaults rather than
        // being pinned by an empty object this adapter invented.
        assert!(gemini_realtime_input_config(&RealtimeConfig::default()).is_none());
    }

    #[test]
    fn interrupted_translates_to_response_cancelled() {
        let raw = json!({ "serverContent": { "interrupted": true } }).to_string();
        let events = GeminiRealtimeSession::translate_event_static(&raw).unwrap();

        assert!(
            matches!(events.as_slice(), [ServerEvent::ResponseCancelled { .. }]),
            "barge-in must surface as a cancellation, got {events:?}"
        );
    }

    #[test]
    fn interrupted_frame_yields_no_playback_audio() {
        // A new generation only begins in response to later client input, so
        // `modelTurn` content arriving with `interrupted` belongs to the
        // abandoned generation. Emitting it would hand the consumer a purge
        // followed by one last stale chunk to enqueue — the exact audio the
        // purge exists to remove.
        let raw = json!({
            "serverContent": {
                "interrupted": true,
                "modelTurn": { "parts": [
                    { "inlineData": { "data": "AAAA" } },
                    { "text": "…as I was saying" }
                ] }
            }
        })
        .to_string();
        let events = GeminiRealtimeSession::translate_event_static(&raw).unwrap();

        assert!(
            matches!(events.as_slice(), [ServerEvent::ResponseCancelled { .. }]),
            "an interrupted frame must leave playback empty, got {events:?}"
        );
    }

    #[test]
    fn interrupted_frame_still_surfaces_caller_speech() {
        // Input transcription is ordered independently of model-turn messages,
        // so suppressing the abandoned generation must not also drop what the
        // caller said to cause the interruption.
        let raw = json!({
            "serverContent": {
                "interrupted": true,
                "modelTurn": { "parts": [{ "inlineData": { "data": "AAAA" } }] },
                "inputTranscription": { "text": "actually, make it ravioli" }
            }
        })
        .to_string();
        let events = GeminiRealtimeSession::translate_event_static(&raw).unwrap();

        assert!(
            events.iter().any(|e| matches!(e, ServerEvent::ResponseCancelled { .. })),
            "expected a cancellation, got {events:?}"
        );
        assert!(
            events.iter().any(|e| matches!(e, ServerEvent::InputTranscriptDelta { .. })),
            "caller speech must survive the suppression, got {events:?}"
        );
        assert!(
            !events.iter().any(|e| matches!(e, ServerEvent::AudioDelta { .. })),
            "abandoned model audio must not survive, got {events:?}"
        );
    }

    #[test]
    fn tool_call_cancellation_is_translated() {
        let raw = json!({ "toolCallCancellation": { "ids": ["call_1", "call_2"] } }).to_string();
        let events = GeminiRealtimeSession::translate_event_static(&raw).unwrap();

        match events.as_slice() {
            [ServerEvent::ToolCallCancelled { call_ids }] => {
                let ids: Vec<&str> = call_ids.iter().map(|id| id.as_str()).collect();
                assert_eq!(ids, ["call_1", "call_2"]);
            }
            other => panic!("expected one ToolCallCancelled, got {other:?}"),
        }
    }

    #[test]
    fn static_path_reports_voice_activity_with_its_offset() {
        // `voiceActivity` carries its own offset, so the session-less path
        // reports it exactly like the session path.
        let raw = r#"{"serverContent":{"speechState":"SPEECH"},"voiceActivity":{"type":"ACTIVITY_START","audioOffset":"7.280s"}}"#;
        let events = GeminiRealtimeSession::translate_event_static(raw).unwrap();
        assert!(
            matches!(events.as_slice(), [ServerEvent::SpeechStarted { audio_start_ms: 7280, .. }]),
            "{events:?}"
        );

        let raw = r#"{"voiceActivity":{"type":"ACTIVITY_END","audioOffset":"10.640s"}}"#;
        let events = GeminiRealtimeSession::translate_event_static(raw).unwrap();
        assert!(
            matches!(events.as_slice(), [ServerEvent::SpeechStopped { audio_end_ms: 10640, .. }]),
            "{events:?}"
        );
    }

    #[test]
    fn usage_metadata_frame_translates_to_usage() {
        let raw = json!({
            "usageMetadata": {
                "promptTokenCount": 120,
                "cachedContentTokenCount": 40,
                "responseTokenCount": 60,
                "toolUsePromptTokenCount": 10,
                "thoughtsTokenCount": 25,
                "totalTokenCount": 205,
                "promptTokensDetails": [
                    { "modality": "TEXT", "tokenCount": 80 },
                    { "modality": "AUDIO", "tokenCount": 40 }
                ],
                "cacheTokensDetails": [{ "modality": "TEXT", "tokenCount": 40 }],
                "responseTokensDetails": [{ "modality": "AUDIO", "tokenCount": 60 }],
                "toolUsePromptTokensDetails": [{ "modality": "TEXT", "tokenCount": 10 }],
                // Preview surface: an unknown field must not break parsing.
                "someFutureField": { "nested": true }
            }
        })
        .to_string();
        let events = GeminiRealtimeSession::translate_event_static(&raw).unwrap();

        match events.as_slice() {
            [ServerEvent::Usage { usage, .. }] => {
                assert_eq!(usage.prompt_token_count, Some(120));
                assert_eq!(usage.cached_content_token_count, Some(40));
                assert_eq!(usage.response_token_count, Some(60));
                assert_eq!(usage.tool_use_prompt_token_count, Some(10));
                assert_eq!(usage.thoughts_token_count, Some(25));
                assert_eq!(usage.total_token_count, Some(205));
                let prompt = usage.prompt_tokens_details.as_deref().expect("prompt details");
                assert_eq!(prompt.len(), 2);
                assert_eq!(prompt[0].modality.as_deref(), Some("TEXT"));
                assert_eq!(prompt[0].token_count, Some(80));
                assert_eq!(prompt[1].modality.as_deref(), Some("AUDIO"));
                assert_eq!(prompt[1].token_count, Some(40));
                let response = usage.response_tokens_details.as_deref().expect("response details");
                assert_eq!(response.len(), 1);
                assert_eq!(response[0].modality.as_deref(), Some("AUDIO"));
                assert_eq!(response[0].token_count, Some(60));
                let cache = usage.cache_tokens_details.as_deref().expect("cache details");
                assert_eq!(cache.len(), 1);
                assert_eq!(cache[0].token_count, Some(40));
                let tool_use =
                    usage.tool_use_prompt_tokens_details.as_deref().expect("tool-use details");
                assert_eq!(tool_use.len(), 1);
                assert_eq!(tool_use[0].token_count, Some(10));
            }
            other => panic!("expected one Usage, got {other:?}"),
        }
    }

    #[test]
    fn usage_metadata_absent_counts_are_none_not_zero() {
        // Google sends only what applies; a missing count is unknown, and
        // reporting it as zero would corrupt cost proof downstream.
        let raw = json!({ "usageMetadata": { "totalTokenCount": 12 } }).to_string();
        let events = GeminiRealtimeSession::translate_event_static(&raw).unwrap();

        match events.as_slice() {
            [ServerEvent::Usage { usage, .. }] => {
                assert_eq!(usage.total_token_count, Some(12));
                assert_eq!(usage.prompt_token_count, None);
                assert_eq!(usage.response_token_count, None);
                assert_eq!(usage.cached_content_token_count, None);
                assert_eq!(usage.thoughts_token_count, None);
                assert_eq!(usage.tool_use_prompt_token_count, None);
                assert!(usage.prompt_tokens_details.is_none());
                assert!(usage.response_tokens_details.is_none());
            }
            other => panic!("expected one Usage, got {other:?}"),
        }
    }

    #[test]
    fn usage_metadata_riding_server_content_arrives_with_the_turn() {
        // `usageMetadata` sits outside the `messageType` union: a turn-final
        // frame carries the turn's content and its counts together, and the
        // translator must surface both, not just the first arm that matches.
        let raw = json!({
            "serverContent": { "turnComplete": true },
            "usageMetadata": {
                "promptTokenCount": 50,
                "responseTokenCount": 20,
                "totalTokenCount": 70
            }
        })
        .to_string();
        let events = GeminiRealtimeSession::translate_event_static(&raw).unwrap();

        match events.as_slice() {
            [ServerEvent::ResponseDone { .. }, ServerEvent::Usage { usage, .. }] => {
                assert_eq!(usage.prompt_token_count, Some(50));
                assert_eq!(usage.response_token_count, Some(20));
                assert_eq!(usage.total_token_count, Some(70));
            }
            other => panic!("expected ResponseDone then Usage, got {other:?}"),
        }
    }

    #[test]
    fn usage_metadata_mistyped_counts_are_a_protocol_error() {
        // The counts feed cost proof: a mistyped block must fail loudly
        // rather than silently drop billing data.
        let raw = json!({ "usageMetadata": { "promptTokenCount": "lots" } }).to_string();
        let result = GeminiRealtimeSession::translate_event_static(&raw);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Malformed usageMetadata"));
    }

    #[test]
    fn waiting_for_input_surfaces_model_waiting() {
        let raw = json!({ "serverContent": { "waitingForInput": true } }).to_string();
        let events = GeminiRealtimeSession::translate_event_static(&raw).unwrap();

        assert!(
            matches!(events.as_slice(), [ServerEvent::ModelWaitingForInput { .. }]),
            "waitingForInput must surface as ModelWaitingForInput, got {events:?}"
        );
    }

    #[test]
    fn generation_complete_surfaces_event() {
        // Previously only recorded in the frame-shape log, never surfaced.
        let raw = json!({ "serverContent": { "generationComplete": true } }).to_string();
        let events = GeminiRealtimeSession::translate_event_static(&raw).unwrap();

        assert!(
            matches!(events.as_slice(), [ServerEvent::GenerationComplete { .. }]),
            "generationComplete must surface as GenerationComplete, got {events:?}"
        );
    }

    #[test]
    fn interaction_status_maps_both_known_values() {
        for (wire, expected) in
            [("IDLE", InteractionStatus::Idle), ("IN_PROGRESS", InteractionStatus::InProgress)]
        {
            let raw = json!({ "serverContent": { "interactionStatus": wire } }).to_string();
            let events = GeminiRealtimeSession::translate_event_static(&raw).unwrap();

            match events.as_slice() {
                [ServerEvent::InteractionStatus { status, .. }] => {
                    assert_eq!(*status, expected, "wire value {wire}");
                }
                other => panic!("expected one InteractionStatus, got {other:?}"),
            }
        }
    }

    #[test]
    fn unknown_interaction_status_parses_as_unknown() {
        // A new provider status must not fail the frame or drop the turn's
        // other signals — it degrades to present-but-unrecognized.
        let raw = json!({
            "serverContent": {
                "turnComplete": true,
                "interactionStatus": "SOME_FUTURE_STATE"
            }
        })
        .to_string();
        let events = GeminiRealtimeSession::translate_event_static(&raw).unwrap();

        match events.as_slice() {
            [ServerEvent::ResponseDone { .. }, ServerEvent::InteractionStatus { status, .. }] => {
                assert_eq!(*status, InteractionStatus::Unknown);
            }
            other => panic!("expected ResponseDone then InteractionStatus, got {other:?}"),
        }
    }

    #[test]
    fn turn_signals_absent_keeps_previous_frame_shape() {
        // All-absent case: a turn-final frame without the new fields must
        // keep exactly the events it produced before they existed.
        let raw = json!({ "serverContent": { "turnComplete": true } }).to_string();
        let events = GeminiRealtimeSession::translate_event_static(&raw).unwrap();

        assert!(
            matches!(events.as_slice(), [ServerEvent::ResponseDone { .. }]),
            "absent signals must add no events, got {events:?}"
        );
    }

    #[test]
    fn false_turn_flags_emit_nothing() {
        // Present-but-false is the server saying "not this"; only `true`
        // is a signal. An otherwise empty frame still degrades to Unknown.
        let raw = json!({
            "serverContent": { "waitingForInput": false, "generationComplete": false }
        })
        .to_string();
        let events = GeminiRealtimeSession::translate_event_static(&raw).unwrap();

        assert!(
            matches!(events.as_slice(), [ServerEvent::Unknown]),
            "false flags must emit nothing, got {events:?}"
        );
    }

    #[test]
    fn combined_turn_signal_fixture_surfaces_all_three() {
        let raw = json!({
            "serverContent": {
                "turnComplete": true,
                "waitingForInput": true,
                "generationComplete": true,
                "interactionStatus": "IN_PROGRESS"
            }
        })
        .to_string();
        let events = GeminiRealtimeSession::translate_event_static(&raw).unwrap();

        match events.as_slice() {
            [
                ServerEvent::ResponseDone { .. },
                ServerEvent::ModelWaitingForInput { .. },
                ServerEvent::GenerationComplete { .. },
                ServerEvent::InteractionStatus { status, .. },
            ] => {
                assert_eq!(*status, InteractionStatus::InProgress);
            }
            other => panic!("expected all four turn signals in order, got {other:?}"),
        }

        // The wire tags consumers match on, pinned: a rename here breaks
        // every downstream deserializer silently.
        let tags: Vec<String> = events
            .iter()
            .map(|e| {
                serde_json::to_value(e)
                    .expect("event serializes")
                    .get("type")
                    .and_then(|t| t.as_str())
                    .expect("tagged")
                    .to_owned()
            })
            .collect();
        assert_eq!(
            tags,
            [
                "response.done",
                "response.waiting_for_input",
                "response.generation_complete",
                "session.interaction_status"
            ]
        );
    }

    #[test]
    fn go_away_translates_to_planned_rotation() {
        let raw = json!({ "goAway": { "timeLeft": "10s" } }).to_string();
        let events = GeminiRealtimeSession::translate_event_static(&raw).unwrap();

        match events.as_slice() {
            [ServerEvent::PlannedRotation { time_left }] => {
                assert_eq!(time_left.as_deref(), Some("10s"));
            }
            other => panic!("expected PlannedRotation, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_duration_string() {
        use std::time::Duration;
        assert_eq!(parse_duration_string("15s"), Some(Duration::from_secs(15)));
        assert_eq!(parse_duration_string(" 15s "), Some(Duration::from_secs(15)));
        assert_eq!(parse_duration_string("500ms"), Some(Duration::from_millis(500)));
        assert_eq!(parse_duration_string("2m"), Some(Duration::from_secs(120)));
        assert_eq!(parse_duration_string("1.5s"), Some(Duration::from_millis(1500)));
        assert_eq!(parse_duration_string("0.5s"), Some(Duration::from_millis(500)));
        assert_eq!(parse_duration_string("100"), Some(Duration::from_secs(100)));
        // Panic-safety checks for negative, NaN, infinity, overflow, and garbage
        assert_eq!(parse_duration_string("-5s"), None);
        assert_eq!(parse_duration_string("-100"), None);
        assert_eq!(parse_duration_string("NaNs"), None);
        assert_eq!(parse_duration_string("infs"), None);
        assert_eq!(parse_duration_string("-infs"), None);
        assert_eq!(parse_duration_string("1e999s"), None);
        assert_eq!(parse_duration_string(""), None);
        assert_eq!(parse_duration_string("invalid"), None);
    }

    #[test]
    fn go_away_without_time_left_translates_to_planned_rotation() {
        let raw = json!({ "goAway": {} }).to_string();
        let events = GeminiRealtimeSession::translate_event_static(&raw).unwrap();

        match events.as_slice() {
            [ServerEvent::PlannedRotation { time_left }] => {
                assert_eq!(time_left.as_deref(), None);
            }
            other => panic!("expected PlannedRotation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn go_away_broadcasts_lifecycle_replacement_deadline() {
        use crate::session::RealtimeLifecycle;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _server = tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let _ = tokio_tungstenite::accept_async(stream).await;
            }
        });
        let (ws_stream, _) =
            tokio_tungstenite::connect_async(format!("ws://{}", addr)).await.unwrap();
        let (_sink, source) = ws_stream.split();
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let writer_task = tokio::spawn(async {});

        let session = GeminiRealtimeSession::new_for_test(
            "test-lifecycle-session".into(),
            format!("ws://{}", addr),
            "models/test".into(),
            tx,
            writer_task,
            source,
        );

        assert!(session.lifecycle().is_some());
        let deadline_rx = session.subscribe_replacement_deadline();
        assert_eq!(*deadline_rx.borrow(), None);

        let raw = json!({ "goAway": { "timeLeft": "15s" } }).to_string();
        let _ = session.translate_gemini_event(&raw).unwrap();

        let deadline = *deadline_rx.borrow();
        assert!(deadline.is_some());
        let remaining = deadline.unwrap().saturating_duration_since(std::time::Instant::now());
        assert!(remaining <= std::time::Duration::from_secs(16));
        assert!(remaining >= std::time::Duration::from_secs(10));
    }

    #[tokio::test]
    async fn go_away_without_valid_time_left_does_not_fabricate_deadline() {
        use crate::session::RealtimeLifecycle;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _server = tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let _ = tokio_tungstenite::accept_async(stream).await;
            }
        });
        let (ws_stream, _) =
            tokio_tungstenite::connect_async(format!("ws://{}", addr)).await.unwrap();
        let (_sink, source) = ws_stream.split();
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let writer_task = tokio::spawn(async {});

        let session = GeminiRealtimeSession::new_for_test(
            "test-lifecycle-session".into(),
            format!("ws://{}", addr),
            "models/test".into(),
            tx,
            writer_task,
            source,
        );

        let deadline_rx = session.subscribe_replacement_deadline();

        // 1. Missing timeLeft
        let raw = json!({ "goAway": {} }).to_string();
        let _ = session.translate_gemini_event(&raw).unwrap();
        assert_eq!(*deadline_rx.borrow(), None);

        // 2. Unparseable timeLeft
        let raw = json!({ "goAway": { "timeLeft": "garbage" } }).to_string();
        let _ = session.translate_gemini_event(&raw).unwrap();
        assert_eq!(*deadline_rx.borrow(), None);

        // 3. Negative timeLeft
        let raw = json!({ "goAway": { "timeLeft": "-10s" } }).to_string();
        let _ = session.translate_gemini_event(&raw).unwrap();
        assert_eq!(*deadline_rx.borrow(), None);
    }

    #[test]
    fn test_call_names_map_fifo_eviction() {
        let mut map = CallNamesMap::new();
        for i in 0..1005 {
            map.insert(format!("call_{i}"), format!("tool_{i}"));
        }
        assert_eq!(map.len(), 1000);
        assert!(map.remove("call_0").is_none());
        assert!(map.remove("call_4").is_none());
        assert_eq!(map.remove("call_5"), Some("tool_5".to_string()));
        assert_eq!(map.remove("call_1004"), Some("tool_1004".to_string()));
    }

    #[tokio::test]
    async fn gemini_session_rejects_stale_tool_response_from_prior_generation_without_wire_writes()
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _server = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    if let Ok(ws) = tokio_tungstenite::accept_async(stream).await {
                        let (_sink, mut stream) = ws.split();
                        while stream.next().await.is_some() {}
                    }
                });
            }
        });

        let ws0 = tokio_tungstenite::connect_async(format!("ws://{}", addr)).await.unwrap().0;
        let (_sink0, source0) = ws0.split();
        let (tx0, _rx0) = tokio::sync::mpsc::channel(10);
        let writer_task0 = tokio::spawn(async {});

        let session0 = GeminiRealtimeSession::new_for_test(
            "session-gen-0".to_string(),
            format!("ws://{}", addr),
            "models/gemini-3.1-flash-live-preview".to_string(),
            tx0,
            writer_task0,
            source0,
        );

        let ws1 = tokio_tungstenite::connect_async(format!("ws://{}", addr)).await.unwrap().0;
        let (_sink1, source1) = ws1.split();
        let (tx1, mut rx1) = tokio::sync::mpsc::channel(10);
        let writer_task1 = tokio::spawn(async {});

        let session1 = GeminiRealtimeSession::new_for_test(
            "session-gen-1".to_string(),
            format!("ws://{}", addr),
            "models/gemini-3.1-flash-live-preview".to_string(),
            tx1,
            writer_task1,
            source1,
        );

        // Receive FunctionCallDone on Session 0
        let fc_raw = json!({
            "toolCall": {
                "functionCalls": [{
                    "name": "lookup_user",
                    "id": "call_gen0_789",
                    "args": {}
                }]
            }
        })
        .to_string();

        let events0 = session0.translate_gemini_event(&fc_raw).unwrap();
        assert_eq!(events0.len(), 1);

        // Session 0 has call_gen0_789 in call_names; Session 1 does NOT
        assert_eq!(session0.call_names_count(), 1);
        assert_eq!(session1.call_names_count(), 0);

        // Attempt send_tool_response for call_gen0_789 on Session 1
        let tool_response = ToolResponse {
            call_id: "call_gen0_789".to_string(),
            output: json!({ "user": "alice" }),
            scheduling: None,
        };

        let err = session1.send_tool_response(tool_response).await.unwrap_err();
        assert!(err.to_string().contains("Missing tool name for call_id 'call_gen0_789'"));

        // Verify zero messages reached Session 1's outbound tx queue
        assert!(
            rx1.try_recv().is_err(),
            "Zero tool response messages must reach Session 1's write queue"
        );
    }

    /// Scheduling rides the function response it belongs to: a declared
    /// WHEN_IDLE reaches the wire verbatim, and an unset scheduling omits
    /// the key so existing integrations see byte-identical frames.
    #[tokio::test]
    async fn tool_response_scheduling_reaches_the_wire_verbatim() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _server = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    if let Ok(ws) = tokio_tungstenite::accept_async(stream).await {
                        let (_sink, mut stream) = ws.split();
                        while stream.next().await.is_some() {}
                    }
                });
            }
        });

        let ws = tokio_tungstenite::connect_async(format!("ws://{addr}")).await.unwrap().0;
        let (_sink, source) = ws.split();
        let (tx, mut rx) = tokio::sync::mpsc::channel(10);
        let writer_task = tokio::spawn(async {});

        let session = GeminiRealtimeSession::new_for_test(
            "session-scheduling".to_string(),
            format!("ws://{addr}"),
            "models/gemini-3.1-flash-live-preview".to_string(),
            tx,
            writer_task,
            source,
        );

        for call_id in ["call_sched_1", "call_sched_2"] {
            let fc_raw = json!({
                "toolCall": {
                    "functionCalls": [{ "name": "probe", "id": call_id, "args": {} }]
                }
            })
            .to_string();
            assert_eq!(session.translate_gemini_event(&fc_raw).unwrap().len(), 1);
        }

        session
            .send_tool_response(
                ToolResponse::new("call_sched_1", json!({ "result": "ok" }))
                    .with_scheduling(FunctionResponseScheduling::WhenIdle),
            )
            .await
            .unwrap();
        let frame = rx.try_recv().expect("scheduled response must reach the write queue");
        let wire: Value = match frame {
            Message::Text(text) => serde_json::from_str(&text).unwrap(),
            other => panic!("expected a text frame, got {other:?}"),
        };
        assert_eq!(
            wire["toolResponse"]["functionResponses"][0]["scheduling"],
            json!("WHEN_IDLE"),
            "{wire}"
        );

        session
            .send_tool_response(ToolResponse::new("call_sched_2", json!({ "result": "ok" })))
            .await
            .unwrap();
        let frame = rx.try_recv().expect("unscheduled response must reach the write queue");
        let wire: Value = match frame {
            Message::Text(text) => serde_json::from_str(&text).unwrap(),
            other => panic!("expected a text frame, got {other:?}"),
        };
        assert!(wire["toolResponse"]["functionResponses"][0].get("scheduling").is_none(), "{wire}");
    }

    #[tokio::test]
    async fn test_tool_call_cancellation_evicts_call_names() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let _ = tokio_tungstenite::accept_async(stream).await;
            }
        });

        let ws = tokio_tungstenite::connect_async(format!("ws://{}", addr)).await.unwrap().0;
        let (_sink, source) = ws.split();
        let (tx, _rx) = tokio::sync::mpsc::channel(10);
        let writer_task = tokio::spawn(async {});

        let session = GeminiRealtimeSession::new_for_test(
            "test-session".to_string(),
            format!("ws://{}", addr),
            "models/gemini-3.1-flash-live-preview".to_string(),
            tx,
            writer_task,
            source,
        );

        let fc_raw = json!({
            "toolCall": {
                "functionCalls": [{
                    "name": "my_tool",
                    "id": "call_123",
                    "args": {}
                }]
            }
        })
        .to_string();

        let _ = session.translate_gemini_event(&fc_raw).unwrap();
        assert_eq!(session.call_names_count(), 1);

        let cancel_raw = json!({
            "toolCallCancellation": {
                "ids": ["call_123"]
            }
        })
        .to_string();

        let _ = session.translate_gemini_event(&cancel_raw).unwrap();
        assert_eq!(session.call_names_count(), 0);

        server.abort();
    }

    #[test]
    fn cancellation_dto_tolerates_fields_google_adds_later() {
        // Gemini Live is a Preview surface. A strict DTO would refuse the whole
        // frame over a field this adapter does not need, taking a live call
        // down for an additive provider change.
        let raw = json!({
            "toolCallCancellation": {
                "ids": ["call_1"],
                "someFutureField": { "nested": true }
            }
        })
        .to_string();
        let events = GeminiRealtimeSession::translate_event_static(&raw).unwrap();

        match events.as_slice() {
            [ServerEvent::ToolCallCancelled { call_ids }] => {
                assert_eq!(call_ids[0].as_str(), "call_1");
            }
            other => panic!("expected one ToolCallCancelled, got {other:?}"),
        }
    }

    #[test]
    fn malformed_cancellation_is_a_protocol_error_not_a_silent_drop() {
        // `ids` present but the wrong shape means the adapter misunderstands
        // the frame. Failing loudly beats treating a real withdrawal as absent.
        let raw = json!({ "toolCallCancellation": { "ids": "call_1" } }).to_string();
        let err = GeminiRealtimeSession::translate_event_static(&raw)
            .expect_err("a non-array ids field must not parse");
        assert!(
            err.to_string().contains("toolCallCancellation"),
            "the error should name the frame, got {err}"
        );
    }

    #[test]
    fn empty_ids_are_dropped_rather_than_carried() {
        // The typed id makes an empty value unrepresentable, so a provider that
        // sends one loses that entry instead of handing a consumer an id that
        // can never match an outstanding call.
        let raw = json!({ "toolCallCancellation": { "ids": ["", "  ", "call_1"] } }).to_string();
        let events = GeminiRealtimeSession::translate_event_static(&raw).unwrap();

        match events.as_slice() {
            [ServerEvent::ToolCallCancelled { call_ids }] => {
                assert_eq!(call_ids.len(), 1, "only the real id survives");
                assert_eq!(call_ids[0].as_str(), "call_1");
            }
            other => panic!("expected one ToolCallCancelled, got {other:?}"),
        }
    }

    #[test]
    fn empty_tool_call_cancellation_is_not_an_event() {
        // An empty id list withdraws nothing. Emitting for it would make a
        // consumer that logs or alarms on cancellation fire on a no-op.
        for raw in [
            json!({ "toolCallCancellation": { "ids": [] } }).to_string(),
            json!({ "toolCallCancellation": {} }).to_string(),
        ] {
            let events = GeminiRealtimeSession::translate_event_static(&raw).unwrap();
            assert!(
                !events.iter().any(|e| matches!(e, ServerEvent::ToolCallCancelled { .. })),
                "no ids means nothing was withdrawn, got {events:?}"
            );
        }
    }

    #[test]
    fn absent_or_false_interrupted_emits_no_cancellation() {
        for raw in [
            json!({ "serverContent": { "turnComplete": true } }).to_string(),
            json!({ "serverContent": { "interrupted": false, "turnComplete": true } }).to_string(),
        ] {
            let events = GeminiRealtimeSession::translate_event_static(&raw).unwrap();
            assert!(
                !events.iter().any(|e| matches!(e, ServerEvent::ResponseCancelled { .. })),
                "only a true `interrupted` is a barge-in, got {events:?}"
            );
        }
    }

    #[test]
    fn test_normalize_model_id() {
        assert_eq!(
            normalize_model_id("gemini-2.5-flash-native-audio"),
            "models/gemini-2.5-flash-native-audio"
        );
        assert_eq!(
            normalize_model_id("models/gemini-3.1-flash-live-preview"),
            "models/gemini-3.1-flash-live-preview"
        );
        assert_eq!(
            normalize_model_id(
                "projects/123/locations/us-central1/publishers/google/models/gemini"
            ),
            "projects/123/locations/us-central1/publishers/google/models/gemini"
        );
    }

    #[tokio::test]
    async fn test_session_resumption_update_stores_last_resume_handle_privately() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _server = tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let _ = tokio_tungstenite::accept_async(stream).await;
            }
        });

        let ws = tokio_tungstenite::connect_async(format!("ws://{}", addr)).await.unwrap().0;
        let (_sink, source) = ws.split();
        let (tx, _rx) = tokio::sync::mpsc::channel(10);
        let writer_task = tokio::spawn(async {});

        let session = GeminiRealtimeSession::new_for_test(
            "test-session".to_string(),
            format!("ws://{}", addr),
            "models/gemini-3.8-live".to_string(),
            tx,
            writer_task,
            source,
        );

        let raw = json!({
            "sessionResumptionUpdate": {
                "resumable": true,
                "newHandle": "test_handle_abc123"
            }
        })
        .to_string();

        let events = session.translate_gemini_event(&raw).unwrap();
        assert!(events.is_empty());
        assert_eq!(session.last_resume_handle(), Some("test_handle_abc123".to_string()));
    }

    #[tokio::test]
    async fn test_candidate_setup_uses_provider_cached_handle_not_stale_config_extra() {
        let setup_received_handle = Arc::new(parking_lot::Mutex::new(None::<String>));
        let setup_capture = Arc::clone(&setup_received_handle);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _server = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let setup_capture = Arc::clone(&setup_capture);
                tokio::spawn(async move {
                    if let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await {
                        if let Some(Ok(Message::Text(msg))) = ws.next().await
                            && let Ok(val) = serde_json::from_str::<Value>(&msg)
                        {
                            let handle = val
                                .get("setup")
                                .and_then(|s| s.get("sessionResumption"))
                                .and_then(|r| r.get("handle"))
                                .and_then(|h| h.as_str())
                                .map(|s| s.to_string());
                            *setup_capture.lock() = handle;
                        }
                        let setup_complete = json!({ "setupComplete": {} });
                        let _ = ws.send(Message::Text(setup_complete.to_string().into())).await;
                    }
                });
            }
        });

        let ws = tokio_tungstenite::connect_async(format!("ws://{}", addr)).await.unwrap().0;
        let (_sink, source) = ws.split();
        let (tx, _rx) = tokio::sync::mpsc::channel(10);
        let writer_task = tokio::spawn(async {});

        let session = GeminiRealtimeSession::new_for_test(
            "test-session".to_string(),
            format!("ws://{}", addr),
            "models/gemini-3.8-live".to_string(),
            tx,
            writer_task,
            source,
        );

        // Simulate provider caching fresh handle H2 ("fresh_provider_h2")
        *session.last_resume_handle.lock() =
            Some(("fresh_provider_h2".to_string(), std::time::Instant::now()));

        // Construct generic config containing stale handle H1 ("stale_config_h1") in config.extra
        let stale_config = RealtimeConfig {
            extra: Some(
                json!({ "resumeHandle": "stale_config_h1", "resumeToken": "stale_config_h1" }),
            ),
            ..Default::default()
        };

        use std::num::NonZeroU32;
        use std::time::Duration;

        let recovery = session.recovery().unwrap();
        let cause = RecoveryCause::UnexpectedEof;
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let context =
            RecoveryContext::new(NonZeroU32::new(1).unwrap(), &cause, &stale_config, deadline);

        let _recovered = recovery.recover(context).await.unwrap();

        let received = setup_received_handle.lock().clone();
        assert_eq!(received, Some("fresh_provider_h2".to_string()));
    }

    /// The whole point of the hand-written `Debug`: a derived one printed this
    /// key verbatim, and the backend is held by the session for the life of the
    /// call.
    #[test]
    fn debug_never_prints_the_studio_api_key() {
        let backend = GeminiLiveBackend::studio("AIzaSy-super-secret-key-value");

        let rendered = format!("{backend:?}");

        assert!(
            !rendered.contains("AIzaSy-super-secret-key-value"),
            "Debug leaked the API key: {rendered}"
        );
        assert!(rendered.contains("<redacted"), "expected a redaction marker: {rendered}");
    }

    /// An empty key and a wrong key are different bugs, so the length survives
    /// redaction.
    #[test]
    fn debug_keeps_the_key_length_so_an_empty_key_is_still_diagnosable() {
        assert!(format!("{:?}", GeminiLiveBackend::studio("")).contains("<redacted 0 bytes>"));
        assert!(format!("{:?}", GeminiLiveBackend::studio("abcd")).contains("<redacted 4 bytes>"));
    }

    /// Redaction must not cost the fields that make a log line useful.
    #[test]
    fn debug_still_shows_the_endpoint_and_variant() {
        let backend =
            GeminiLiveBackend::studio("secret").with_endpoint_url("ws://127.0.0.1:9999/mock");

        let rendered = format!("{backend:?}");

        assert!(rendered.contains("Studio"), "variant should be visible: {rendered}");
        assert!(rendered.contains("ws://127.0.0.1:9999/mock"), "endpoint lost: {rendered}");
        assert!(!rendered.contains("secret"), "Debug leaked the API key: {rendered}");
    }

    /// ADK attaches no Google credential to a custom host, so whatever
    /// authenticates that host belongs to the caller -- and the usual place to
    /// put it is the query string.
    #[test]
    fn debug_withholds_whatever_a_custom_endpoint_carries_in_its_query() {
        let backend = GeminiLiveBackend::studio("k")
            .with_endpoint_url("wss://proxy.internal/live?access_token=caller-owned-secret");

        let rendered = format!("{backend:?}");

        assert!(
            !rendered.contains("caller-owned-secret"),
            "Debug leaked the endpoint's query string: {rendered}"
        );
        assert!(
            rendered.contains("wss://proxy.internal/live"),
            "the addressable part must survive: {rendered}"
        );
        assert!(
            rendered.contains("?<redacted>"),
            "a withheld query must still be visible as present: {rendered}"
        );
    }

    /// A session wired to a mock that is never spoken to, so nothing is
    /// recorded on it: no setup, no disconnect. Used for the cases where the
    /// transport is genuinely not the subject.
    async fn session_for_classify() -> GeminiRealtimeSession {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _server = tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let _ = tokio_tungstenite::accept_async(stream).await;
            }
        });

        let ws = tokio_tungstenite::connect_async(format!("ws://{}", addr)).await.unwrap().0;
        let (_sink, source) = ws.split();
        let (tx, _rx) = tokio::sync::mpsc::channel(10);
        let writer_task = tokio::spawn(async {});

        GeminiRealtimeSession::new_for_test(
            "classify-session".to_string(),
            format!("ws://{}", addr),
            "models/gemini-3.1-flash-live-preview".to_string(),
            tx,
            writer_task,
            source,
        )
    }

    /// Drive a real close through a real socket, rather than poking the flags
    /// `classify` reads.
    ///
    /// The mock optionally completes setup, then closes with `code`/`reason`.
    /// The session is drained to end of stream first, so the state `classify`
    /// sees is the state the transport actually produced.
    async fn session_after_server_close(
        complete_setup: bool,
        code: CloseCode,
        reason: &'static str,
    ) -> GeminiRealtimeSession {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _server = tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await
                && let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await
            {
                if complete_setup {
                    let _ = ws
                        .send(Message::Text(json!({ "setupComplete": {} }).to_string().into()))
                        .await;
                }
                let _ =
                    ws.send(Message::Close(Some(CloseFrame { code, reason: reason.into() }))).await;
                let _ = ws.flush().await;
            }
        });

        let ws = tokio_tungstenite::connect_async(format!("ws://{}", addr)).await.unwrap().0;
        let (_sink, source) = ws.split();
        let (tx, _rx) = tokio::sync::mpsc::channel(10);
        let writer_task = tokio::spawn(async {});

        let session = GeminiRealtimeSession::new_for_test(
            "close-session".to_string(),
            format!("ws://{}", addr),
            "models/gemini-3.1-flash-live-preview".to_string(),
            tx,
            writer_task,
            source,
        );

        while session.next_event().await.is_some() {}
        session
    }

    /// The case the change exists for, proved end to end: the provider closed
    /// before it ever accepted the setup, so retrying would re-send the frame
    /// it just refused.
    #[tokio::test]
    async fn a_server_close_before_setup_complete_is_fatal() {
        let session = session_after_server_close(
            false,
            CloseCode::Policy,
            "request contains an invalid model",
        )
        .await;

        assert_eq!(
            session.classify(&RecoveryCause::UnexpectedEof),
            RecoveryDisposition::Fatal,
            "a setup the server refused must not be re-sent"
        );
    }

    /// Failing fast is only worth anything if it preserves the reason. A
    /// recovery attempt would end on the candidate's own pre-setup close and
    /// overwrite this with a generic message.
    #[tokio::test]
    async fn the_refusal_reason_survives_for_the_caller_to_read() {
        let session = session_after_server_close(
            false,
            CloseCode::Policy,
            "request contains an invalid model",
        )
        .await;

        let disconnect = session.disconnect_reason().expect("the close must be recorded");

        assert_eq!(disconnect.code, Some(u16::from(CloseCode::Policy)));
        assert_eq!(disconnect.reason, "request contains an invalid model");
    }

    /// The same close code, 1008, on a session that had completed setup. Gemini
    /// uses it for both, which is why the code alone cannot decide this and the
    /// setup latch has to.
    #[tokio::test]
    async fn the_same_close_after_setup_complete_stays_recoverable() {
        let session =
            session_after_server_close(true, CloseCode::Policy, "The operation was aborted.").await;

        assert_eq!(
            session.classify(&RecoveryCause::UnexpectedEof),
            RecoveryDisposition::Recoverable,
            "an established session that the provider dropped is the recoverable case"
        );
    }

    /// The conservative half of the policy, and the reason `classify` demands a
    /// recorded close rather than just the absence of setup.
    ///
    /// `UnexpectedEof` also reaches `classify` from a stream that simply ended,
    /// which records no code and no reason. That is not evidence the provider
    /// refused anything -- it is the transient transport fault managed recovery
    /// exists for -- so it stays recoverable even before setup completes.
    #[tokio::test]
    async fn a_bare_eof_before_setup_complete_stays_recoverable() {
        let session = session_for_classify().await;

        assert!(
            session.disconnect_reason().is_none(),
            "precondition: no close frame was recorded on this session"
        );
        assert_eq!(
            session.classify(&RecoveryCause::UnexpectedEof),
            RecoveryDisposition::Recoverable,
            "an EOF with no provider close behind it must not be treated as a refusal"
        );
    }

    /// `goAway` can arrive before this connection has completed setup -- it is
    /// the server scheduling a replacement, not refusing a request -- so the
    /// pre-setup guard must not swallow a planned rotation.
    #[tokio::test]
    async fn a_planned_rotation_before_setup_complete_is_still_recoverable() {
        let session = session_for_classify().await;

        assert_eq!(
            session.classify(&RecoveryCause::PlannedRotation { time_left: None }),
            RecoveryDisposition::Recoverable,
            "a planned rotation is recoverable regardless of setup state"
        );
    }

    /// Latched, not sampled. Frames keep arriving after `setupComplete`, and
    /// none of them may quietly return the session to "setup never completed"
    /// and make a mid-session drop unrecoverable.
    #[tokio::test]
    async fn setup_complete_survives_the_frames_that_follow_it() {
        let session = session_for_classify().await;
        session.translate_gemini_event(&json!({ "setupComplete": {} }).to_string()).unwrap();
        session
            .translate_gemini_event(
                &json!({ "sessionResumptionUpdate": { "resumable": false } }).to_string(),
            )
            .unwrap();
        session
            .translate_gemini_event(&json!({ "goAway": { "timeLeft": "10s" } }).to_string())
            .unwrap();

        assert_eq!(
            session.classify(&RecoveryCause::UnexpectedEof),
            RecoveryDisposition::Recoverable,
            "later frames must not clear the setup-complete latch"
        );
    }
}

#[cfg(test)]
mod gemini_38_compatibility_tests {
    use super::*;

    #[test]
    fn test_gemini_38_capabilities() {
        let caps = capabilities_for("models/gemini-3.8-live");
        assert!(!caps.configurable_thinking);
        assert!(caps.affective_dialog);
        assert!(caps.proactive_audio);
        assert!(!caps.cached_content);
        assert!(caps.audio_only_response);
        assert!(caps.async_functions);

        let caps_ext = capabilities_for("models/gemini-3.8-live-extended-thinking");
        assert!(caps_ext.configurable_thinking);
        assert!(caps_ext.affective_dialog);
        assert!(caps_ext.proactive_audio);
        assert!(!caps_ext.cached_content);
        assert!(caps_ext.audio_only_response);
        assert!(caps_ext.async_functions);

        let caps_fallback = capabilities_for("models/unknown-experimental-model");
        assert!(!caps_fallback.configurable_thinking);
        assert!(!caps_fallback.affective_dialog);
        assert!(!caps_fallback.proactive_audio);
        assert!(!caps_fallback.cached_content);
        assert!(caps_fallback.audio_only_response);
        assert!(!caps_fallback.async_functions);

        let caps_31 = capabilities_for("models/gemini-3.1-flash-live-preview");
        assert!(caps_31.configurable_thinking);
        assert!(!caps_31.affective_dialog);
        assert!(!caps_31.proactive_audio);
        assert!(!caps_31.cached_content);
        assert!(!caps_31.audio_only_response);
        assert!(!caps_31.async_functions);
    }

    #[test]
    fn test_setup_emits_proactivity_on_supported_model() {
        let config = RealtimeConfig::default().with_proactive_audio(true);
        let message = GeminiRealtimeSession::build_setup_message(
            "models/gemini-3.8-live",
            config,
            None,
            None,
        )
        .expect("setup builds successfully for 3.8 Live with proactive audio");

        let js = serde_json::to_value(&message).unwrap();
        let setup = js.get("setup").expect("setup field present");
        assert_eq!(setup.get("proactivity"), Some(&json!({ "proactiveAudio": true })));

        // Unset means omitted, not present-but-false: the server must see no
        // proactivity field at all for models that do not honor it.
        let message = GeminiRealtimeSession::build_setup_message(
            "models/gemini-3.8-live",
            RealtimeConfig::default(),
            None,
            None,
        )
        .expect("setup builds successfully");
        let js = serde_json::to_value(&message).unwrap();
        let setup = js.get("setup").expect("setup field present");
        assert!(setup.get("proactivity").is_none());
    }

    #[test]
    fn test_setup_rejects_proactivity_on_excluded_models() {
        for model in ["models/gemini-3.1-flash-live-preview", "models/unknown-experimental-model"] {
            let config = RealtimeConfig::default().with_proactive_audio(true);
            let err =
                GeminiRealtimeSession::build_setup_message(model, config, None, None).unwrap_err();
            assert!(
                err.to_string().contains("does not support proactive audio"),
                "unexpected error for {model}: {err}"
            );
        }
    }

    #[test]
    fn test_setup_emits_affective_dialog_on_supported_model() {
        let config = RealtimeConfig::default().with_affective_dialog(true);
        let message = GeminiRealtimeSession::build_setup_message(
            "models/gemini-3.8-live",
            config,
            None,
            None,
        )
        .expect("setup builds successfully for 3.8 Live with affective dialog");

        let js = serde_json::to_value(&message).unwrap();
        let setup = js.get("setup").expect("setup field present");
        let gen_config = setup.get("generationConfig").expect("generationConfig present");
        assert_eq!(gen_config.get("enableAffectiveDialog"), Some(&json!(true)));
        assert!(setup.get("proactivity").is_none());
    }

    #[test]
    fn test_setup_rejects_affective_dialog_on_excluded_models() {
        for model in ["models/gemini-3.1-flash-live-preview", "models/unknown-experimental-model"] {
            let config = RealtimeConfig::default().with_affective_dialog(true);
            let err =
                GeminiRealtimeSession::build_setup_message(model, config, None, None).unwrap_err();
            assert!(
                err.to_string().contains("does not support affective dialog"),
                "unexpected error for {model}: {err}"
            );
        }
    }

    #[test]
    fn test_gemini_38_extended_thinking_supports_thinking_config() {
        let model = "models/gemini-3.8-live-extended-thinking";
        let config_thinking = RealtimeConfig {
            extra: Some(json!({ "thinking_level": "HIGH" })),
            ..Default::default()
        };
        let msg = GeminiRealtimeSession::build_setup_message(model, config_thinking, None, None)
            .expect("setup builds successfully for extended thinking with thinking_level");

        let js = serde_json::to_value(&msg).unwrap();
        let setup = js.get("setup").expect("setup field present");
        let gen_config = setup.get("generationConfig").expect("generationConfig present");
        assert_eq!(gen_config.get("thinkingConfig"), Some(&json!({ "thinkingLevel": "HIGH" })));
    }

    #[test]
    fn test_gemini_38_setup_serialization_and_tool_blocking_behavior() {
        let adapter = adk_gemini::schema_adapter::GeminiSchemaAdapter::new();
        let cache = adk_core::SchemaCache::new();
        let caps = capabilities_for("models/gemini-3.8-live");

        let tool = ToolDefinition::new("get_weather")
            .with_description("Get weather")
            .with_parameters(json!({
                "type": "object",
                "properties": {
                    "city": { "type": "string" }
                }
            }));

        let compiled_tools =
            convert_tools(Some(vec![tool]), &cache, &adapter, caps).expect("compiles");

        let config = RealtimeConfig::default().with_instruction("Hello");
        let message = GeminiRealtimeSession::build_setup_message(
            "models/gemini-3.8-live",
            config,
            compiled_tools,
            None,
        )
        .expect("setup builds successfully");

        let js = serde_json::to_value(&message).unwrap();
        let setup = js.get("setup").expect("setup field present");

        assert_eq!(setup.get("model").and_then(Value::as_str), Some("models/gemini-3.8-live"));

        let gen_config = setup.get("generationConfig").expect("generationConfig present");
        assert_eq!(gen_config.get("responseModalities"), Some(&json!(["AUDIO"])));
        assert!(gen_config.get("thinkingConfig").is_none());
        assert!(gen_config.get("enableAffectiveDialog").is_none());

        let tools = setup.get("tools").and_then(Value::as_array).expect("tools array");
        let func_decl = &tools[0]["functionDeclarations"][0];
        assert_eq!(func_decl.get("name").and_then(Value::as_str), Some("get_weather"));
        assert_eq!(func_decl.get("behavior").and_then(Value::as_str), Some("BLOCKING"));
        assert!(func_decl.get("parameters").is_some());
    }

    fn setup_generation_config(config: RealtimeConfig) -> Value {
        let message = GeminiRealtimeSession::build_setup_message(
            "models/gemini-3.8-live",
            config,
            None,
            None,
        )
        .expect("setup builds successfully");
        let js = serde_json::to_value(&message).unwrap();
        js.get("setup")
            .expect("setup field present")
            .get("generationConfig")
            .expect("generationConfig present")
            .clone()
    }

    /// The wire pin: a configured output language reaches the setup as
    /// `speechConfig.languageCode`, beside the voice — the session-level
    /// constraint that holds the line's language when the model ignores
    /// the prompt's.
    #[test]
    fn pinned_language_reaches_speech_config_language_code() {
        let gen_config = setup_generation_config(
            RealtimeConfig::default().with_voice("Puck").try_with_language("en-US").unwrap(),
        );

        assert_eq!(
            gen_config.get("speechConfig"),
            Some(&json!({
                "languageCode": "en-US",
                "voiceConfig": {
                    "prebuiltVoiceConfig": { "voiceName": "Puck" }
                }
            }))
        );
    }

    /// A language without a voice still constrains the session: the pin
    /// must not depend on a voice being configured.
    #[test]
    fn pinned_language_without_a_voice_still_constrains_the_session() {
        let gen_config =
            setup_generation_config(RealtimeConfig::default().try_with_language("fr-CA").unwrap());

        assert_eq!(gen_config.get("speechConfig"), Some(&json!({ "languageCode": "fr-CA" })));
    }

    /// The unpinned path must not move: a voice without a language sends
    /// the same `speechConfig` as before (no present-but-null
    /// `languageCode`), and a config with neither sends no `speechConfig`
    /// at all.
    #[test]
    fn unpinned_setups_are_byte_identical_to_before() {
        let voiced = setup_generation_config(RealtimeConfig::default().with_voice("Puck"));
        assert_eq!(
            voiced.get("speechConfig"),
            Some(&json!({
                "voiceConfig": {
                    "prebuiltVoiceConfig": { "voiceName": "Puck" }
                }
            }))
        );

        let bare = setup_generation_config(RealtimeConfig::default());
        assert!(
            bare.get("speechConfig").is_none(),
            "no voice and no language must send no speechConfig, got {bare}"
        );
    }

    #[test]
    fn test_gemini_38_rejects_unsupported_configurations() {
        let model = "models/gemini-3.8-live";

        // 1. Thinking level
        let config_thinking = RealtimeConfig {
            extra: Some(json!({ "thinking_level": "HIGH" })),
            ..Default::default()
        };
        let err = GeminiRealtimeSession::build_setup_message(model, config_thinking, None, None)
            .unwrap_err();
        assert!(err.to_string().contains("does not support configurable thinking"));

        // 2. Cached content (affective dialog is honored on 3.8 — its
        // rejection on excluded models lives in
        // test_setup_rejects_affective_dialog_on_excluded_models).
        let config_cached = RealtimeConfig {
            cached_content: Some("cached-resource-123".to_string()),
            ..Default::default()
        };
        let err = GeminiRealtimeSession::build_setup_message(model, config_cached, None, None)
            .unwrap_err();
        assert!(err.to_string().contains("does not support cached content"));

        // 3. Non-AUDIO response modality (e.g. TEXT)
        let config_text =
            RealtimeConfig { modalities: Some(vec!["text".to_string()]), ..Default::default() };
        let err =
            GeminiRealtimeSession::build_setup_message(model, config_text, None, None).unwrap_err();
        assert!(err.to_string().contains("requires AUDIO response modality"));

        // 4. Provider-neutral modality defaults containing AUDIO (e.g. ["text", "audio"]) are sanitized
        let config_mixed = RealtimeConfig {
            modalities: Some(vec!["text".to_string(), "audio".to_string()]),
            ..Default::default()
        };
        let msg = GeminiRealtimeSession::build_setup_message(model, config_mixed, None, None)
            .expect("setup builds successfully for mixed modalities containing audio");
        let js = serde_json::to_value(&msg).unwrap();
        let setup = js.get("setup").expect("setup field present");
        let gen_config = setup.get("generationConfig").expect("generationConfig present");
        assert_eq!(gen_config.get("responseModalities"), Some(&json!(["AUDIO"])));
    }
}

#[cfg(test)]
mod teardown_tests {
    use super::*;
    use crate::config::{VadConfig, VadMode};
    use adk_core::types::Part;
    use bytes::BytesMut;
    use parking_lot::Mutex as ParkingMutex;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn gemini_close_is_bounded_even_when_channel_is_full() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _server_handle = tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let _ = tokio_tungstenite::accept_async(stream).await;
            }
        });
        let (ws_stream, _) =
            tokio_tungstenite::connect_async(format!("ws://{}", addr)).await.unwrap();
        let (_sink, source) = ws_stream.split();

        let (outbound_tx, _outbound_rx) = mpsc::channel::<Message>(1);
        // Fill channel to capacity
        outbound_tx.try_send(Message::Text("blocker".into())).unwrap();

        let connected = Arc::new(AtomicBool::new(true));
        let cancel_token = CancellationToken::new();
        let writer_cancel = cancel_token.clone();

        // Spawn a writer task that simulates being stuck on a slow/blocked socket send without draining channel
        let writer_task = tokio::spawn(async move {
            tokio::select! {
                _ = writer_cancel.cancelled() => {},
                _ = tokio::time::sleep(Duration::from_secs(60)) => {},
            }
        });

        let (replacement_deadline_tx, replacement_deadline_rx) = tokio::sync::watch::channel(None);

        let session = GeminiRealtimeSession {
            session_id: "test-gemini-session".into(),
            connected,
            cancel_token: Arc::new(Mutex::new(cancel_token)),
            outbound_tx: Arc::new(Mutex::new(outbound_tx)),
            writer_task: Arc::new(Mutex::new(Some(writer_task))),
            receiver: Arc::new(Mutex::new(source)),
            audio_buffer: Arc::new(ParkingMutex::new(BytesMut::new())),
            event_queue: Arc::new(ParkingMutex::new(std::collections::VecDeque::new())),
            frame_log: FrameLog::new(
                "test-gemini-session".into(),
                "studio",
                "models/gemini-3.1-flash-live-preview".into(),
            ),
            last_disconnect: Arc::new(ParkingMutex::new(None)),
            backend: GeminiLiveBackend::studio("test-key").with_endpoint_url("wss://localhost/ws"),
            dialect: GeminiSchemaDialect::OpenApiSubset,
            reconnect_model: "models/gemini-3.1-flash-live-preview".into(),
            last_resume_handle: Arc::new(ParkingMutex::new(None)),
            call_names: Arc::new(ParkingMutex::new(CallNamesMap::new())),
            replacement_deadline_tx,
            replacement_deadline_rx,
            activity_detection: ActivityDetection::Automatic,
            activity_open: Arc::new(AtomicBool::new(false)),
            setup_complete_seen: Arc::new(AtomicBool::new(false)),
            last_usage: Arc::new(ParkingMutex::new(None)),
            speech_active: Arc::new(AtomicBool::new(false)),
        };

        // Execution of close() must complete quickly despite channel full & writer blocked
        let start = std::time::Instant::now();
        let close_result = session.close().await;
        let elapsed = start.elapsed();

        assert!(close_result.is_ok());
        assert!(
            elapsed >= Duration::from_millis(450),
            "close() should wait out channel send timeout (500ms), took {:?}",
            elapsed
        );
        assert!(
            elapsed < Duration::from_secs(3),
            "close() took too long ({:?}); must be bounded under 3s",
            elapsed
        );
        assert!(!session.connected.load(Ordering::SeqCst));
    }

    #[test]
    fn activity_end_serializes_as_documented() {
        let msg = GeminiClientMessage {
            realtime_input: Some(ActivitySignal::End.into_realtime_input()),
            ..Default::default()
        };

        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            json!({ "realtimeInput": { "activityEnd": {} } })
        );
    }

    /// `gemini-3.8-live` ignores images under the deprecated `mediaChunks` (the model says
    /// it sees nothing); under `video` they count as IMAGE tokens and are described.
    #[test]
    fn video_frames_go_out_as_realtime_input_video() {
        assert_eq!(
            serde_json::to_value(video_frame_message("image/jpeg", "AQI=")).unwrap(),
            json!({ "realtimeInput": { "video": { "mimeType": "image/jpeg", "data": "AQI=" } } })
        );
    }

    /// The two frames are separate transitions; emitting both in one message
    /// would describe a turn that started and ended simultaneously.
    #[test]
    fn an_activity_frame_carries_exactly_one_marker() {
        for signal in [ActivitySignal::Start, ActivitySignal::End] {
            let input = signal.into_realtime_input();
            assert_eq!(
                input.activity_start.is_some() as u8 + input.activity_end.is_some() as u8,
                1,
                "{signal:?} should set exactly one marker"
            );
            assert!(input.audio.is_none() && input.video.is_none() && input.text.is_none());
        }
    }

    #[test]
    fn only_vad_mode_none_asks_for_manual_activity_detection() {
        let cases = [
            (None, ActivityDetection::Automatic),
            (Some(VadMode::ServerVad), ActivityDetection::Automatic),
            (Some(VadMode::SemanticVad), ActivityDetection::Automatic),
            (Some(VadMode::None), ActivityDetection::Manual),
        ];

        for (mode, expected) in cases {
            let config = match mode {
                None => RealtimeConfig::default(),
                Some(mode) => {
                    RealtimeConfig::default().with_vad(VadConfig { mode, ..VadConfig::default() })
                }
            };
            assert_eq!(ActivityDetection::from_config(&config), expected, "mode {mode:?}");
        }
    }

    fn setup_json(config: RealtimeConfig) -> Value {
        let msg = GeminiRealtimeSession::build_setup_message(
            "models/gemini-3.1-flash-live-preview",
            config,
            None,
            None,
        )
        .unwrap();
        serde_json::to_value(&msg).expect("setup serializes")
    }

    /// `VadMode::None` used to be a request Gemini never heard about. This is
    /// the assertion that it now reaches the wire.
    #[test]
    fn manual_mode_disables_automatic_activity_detection_in_setup() {
        let js = setup_json(RealtimeConfig::default().without_vad());

        assert_eq!(
            js["setup"]["realtimeInputConfig"],
            json!({ "automaticActivityDetection": { "disabled": true } })
        );
    }

    /// The default path is the one that must not move. Every configuration
    /// that leaves detection with the server has to serialize exactly as it
    /// did before `realtimeInputConfig` existed, which means the key must be
    /// absent rather than present-and-false.
    #[test]
    fn server_side_detection_setups_are_byte_identical_to_before() {
        let baseline = setup_json(RealtimeConfig::default());

        for config in [
            RealtimeConfig::default(),
            RealtimeConfig::default().with_server_vad(),
            RealtimeConfig::default().with_vad(VadConfig::semantic_vad()),
        ] {
            let js = setup_json(config);
            assert!(
                js["setup"].get("realtimeInputConfig").is_none(),
                "server-side detection must not send realtimeInputConfig, got {js}"
            );
            assert_eq!(js, baseline);
        }
    }

    // ── Session-level behaviour, over a real WebSocket ──────────────────

    use tokio_tungstenite::MaybeTlsStream;
    use tokio_tungstenite::tungstenite::protocol::Role;

    type ServerEnd = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

    /// A session wired to a loopback WebSocket, so tests observe the bytes the
    /// session actually writes rather than a stand-in for them.
    async fn session_on_loopback(
        activity_detection: ActivityDetection,
    ) -> (GeminiRealtimeSession, ServerEnd) {
        session_on_loopback_with_dialect(
            activity_detection,
            adk_gemini::GeminiSchemaDialect::default(),
        )
        .await
    }

    /// As [`session_on_loopback`], with an explicit schema dialect, so a test
    /// can prove the session's own dialect is what reaches the wire.
    async fn session_on_loopback_with_dialect(
        activity_detection: ActivityDetection,
        schema_dialect: adk_gemini::GeminiSchemaDialect,
    ) -> (GeminiRealtimeSession, ServerEnd) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let (client_io, accepted) =
            tokio::join!(tokio::net::TcpStream::connect(addr), listener.accept());
        let client_io = client_io.unwrap();
        let (server_io, _) = accepted.unwrap();

        let client_ws = tokio_tungstenite::WebSocketStream::from_raw_socket(
            MaybeTlsStream::Plain(client_io),
            Role::Client,
            None,
        )
        .await;
        let server_ws =
            tokio_tungstenite::WebSocketStream::from_raw_socket(server_io, Role::Server, None)
                .await;

        let (mut sink, source) = client_ws.split();
        let (outbound_tx, mut outbound_rx) = mpsc::channel(WRITER_CHANNEL_CAPACITY);
        let writer_task = tokio::spawn(async move {
            while let Some(message) = outbound_rx.recv().await {
                if sink.send(message).await.is_err() {
                    break;
                }
            }
        });

        let (replacement_deadline_tx, replacement_deadline_rx) = tokio::sync::watch::channel(None);
        let cancel_token = tokio_util::sync::CancellationToken::new();

        let session = GeminiRealtimeSession {
            session_id: "test-session".to_string(),
            connected: Arc::new(AtomicBool::new(true)),
            cancel_token: Arc::new(Mutex::new(cancel_token)),
            outbound_tx: Arc::new(Mutex::new(outbound_tx)),
            writer_task: Arc::new(Mutex::new(Some(writer_task))),
            receiver: Arc::new(Mutex::new(source)),
            audio_buffer: Arc::new(ParkingMutex::new(BytesMut::new())),
            event_queue: Arc::new(ParkingMutex::new(std::collections::VecDeque::new())),
            frame_log: FrameLog::new("test-session".into(), "studio", "models/test".into()),
            last_disconnect: Arc::new(ParkingMutex::new(None)),
            backend: GeminiLiveBackend::studio("mock-key"),
            dialect: schema_dialect,
            reconnect_model: "models/test".into(),
            last_resume_handle: Arc::new(ParkingMutex::new(None)),
            call_names: Arc::new(ParkingMutex::new(CallNamesMap::new())),
            replacement_deadline_tx,
            replacement_deadline_rx,
            activity_detection,
            activity_open: Arc::new(AtomicBool::new(false)),
            setup_complete_seen: Arc::new(AtomicBool::new(false)),
            last_usage: Arc::new(ParkingMutex::new(None)),
            speech_active: Arc::new(AtomicBool::new(false)),
        };

        (session, server_ws)
    }

    /// The next frame the server sees, as JSON.
    async fn next_frame(server: &mut ServerEnd) -> Value {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(5), server.next())
            .await
            .expect("timed out waiting for a frame")
            .expect("stream ended")
            .expect("websocket error");
        match msg {
            Message::Text(text) => serde_json::from_str(&text).expect("frame is JSON"),
            other => panic!("expected a text frame, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_server_vad_session_offers_no_signaller() {
        let (session, _server) = session_on_loopback(ActivityDetection::Automatic).await;

        assert!(session.activity_signaller().is_none());
        assert_eq!(session.activity_detection(), ActivityDetection::Automatic);
    }

    /// The exclusivity rule is enforced by construction via
    /// `activity_signaller`, but the in-crate path still has to fail loudly
    /// rather than emit a frame the server would reject.
    #[tokio::test]
    async fn activity_frames_are_refused_while_server_detection_is_on() {
        let (session, _server) = session_on_loopback(ActivityDetection::Automatic).await;

        let err = session.send_activity(ActivitySignal::Start).await.unwrap_err();
        assert!(
            matches!(err, RealtimeError::ConfigError(_)),
            "expected a config error, got {err:?}"
        );
        assert!(err.to_string().contains("automatic activity detection is disabled"));
    }

    /// Committing on a server-VAD session flushes the buffered audio and ends the audio
    /// stream, so the server closes the user's turn without waiting for more audio.
    #[tokio::test]
    async fn commit_ends_the_audio_stream_when_the_server_owns_detection() {
        let (session, mut server) = session_on_loopback(ActivityDetection::Automatic).await;

        session.send_audio(&AudioChunk::pcm16_16khz(vec![0; 64])).await.unwrap();
        session.commit_audio().await.unwrap();

        let audio = next_frame(&mut server).await;
        assert_eq!(audio["realtimeInput"]["audio"]["mimeType"], "audio/pcm;rate=16000");
        assert_eq!(
            next_frame(&mut server).await,
            json!({ "realtimeInput": { "audioStreamEnd": true } })
        );
    }

    /// Every commit under server detection re-ends the audio stream: the
    /// server owns the turn boundary, so a repeated commit is not a
    /// redundant edge the way a repeated manual `activityEnd` is.
    #[tokio::test]
    async fn repeated_commit_audio_resends_audio_stream_end_under_server_detection() {
        let (session, mut server) = session_on_loopback(ActivityDetection::Automatic).await;

        session.commit_audio().await.unwrap();
        session.commit_audio().await.unwrap();

        assert_eq!(
            next_frame(&mut server).await,
            json!({ "realtimeInput": { "audioStreamEnd": true } })
        );
        assert_eq!(
            next_frame(&mut server).await,
            json!({ "realtimeInput": { "audioStreamEnd": true } })
        );
    }

    /// In manual mode the client's `activityEnd` closes the turn; commit only flushes.
    #[tokio::test]
    async fn commit_without_open_activity_sends_no_frame_in_manual_mode() {
        let (session, mut server) = session_on_loopback(ActivityDetection::Manual).await;

        session.commit_audio().await.unwrap();
        session.send_text("sentinel").await.unwrap();

        let frame = next_frame(&mut server).await;
        assert_eq!(frame["realtimeInput"]["text"], "sentinel", "commit wrote a frame: {frame}");
    }

    /// `interrupt()` on a server-VAD session must stay a local-only operation:
    /// this is the default configuration and sending anything would be a
    /// protocol violation. The sentinel proves nothing was written, rather
    /// than a timeout only suggesting it.
    #[tokio::test]
    async fn interrupt_writes_nothing_when_the_server_owns_detection() {
        let (session, mut server) = session_on_loopback(ActivityDetection::Automatic).await;

        session.interrupt().await.expect("interrupt is not an error on the default path");
        session.send_text("sentinel").await.unwrap();

        let frame = next_frame(&mut server).await;
        assert_eq!(
            frame["realtimeInput"]["text"], "sentinel",
            "an activity frame preceded the sentinel: {frame}"
        );
    }

    /// The truthfulness fix: in manual mode `interrupt()` is no longer a
    /// local no-op that returns `Ok`.
    #[tokio::test]
    async fn interrupt_emits_activity_start_in_manual_mode() {
        let (session, mut server) = session_on_loopback(ActivityDetection::Manual).await;

        session.interrupt().await.unwrap();

        assert_eq!(
            next_frame(&mut server).await,
            json!({ "realtimeInput": { "activityStart": {} } })
        );
    }

    /// The opening edge of a locally detected turn, reachable without a
    /// downcast to this concrete session.
    #[tokio::test]
    async fn begin_activity_emits_activity_start_in_manual_mode() {
        let (session, mut server) = session_on_loopback(ActivityDetection::Manual).await;

        session.begin_activity().await.unwrap();

        assert_eq!(
            next_frame(&mut server).await,
            json!({ "realtimeInput": { "activityStart": {} } })
        );
    }

    /// Reporting caller speech while the server owns detection is redundant,
    /// not an error: the server already has a better source for it. Failing
    /// here would punish a caller for using the neutral API on a backend that
    /// happens not to need it — and would put a frame on the wire that the
    /// protocol forbids in this mode.
    #[tokio::test]
    async fn begin_activity_writes_nothing_when_the_server_owns_detection() {
        let (session, mut server) = session_on_loopback(ActivityDetection::Automatic).await;

        session.begin_activity().await.expect("redundant, not an error");
        session.send_text("sentinel").await.unwrap();

        let frame = next_frame(&mut server).await;
        assert_eq!(
            frame["realtimeInput"]["text"], "sentinel",
            "an activity frame preceded the sentinel: {frame}"
        );
    }

    /// `begin_activity` and `interrupt` are different statements that Gemini
    /// happens to spell with one frame. Two reports of one turn-start must
    /// still be one frame, or the second is read as a new turn.
    #[tokio::test]
    async fn begin_activity_and_interrupt_are_one_turn_start() {
        let (session, mut server) = session_on_loopback(ActivityDetection::Manual).await;

        session.begin_activity().await.unwrap();
        session.interrupt().await.unwrap();
        session.commit_audio().await.unwrap();

        assert_eq!(
            next_frame(&mut server).await,
            json!({ "realtimeInput": { "activityStart": {} } })
        );
        assert_eq!(
            next_frame(&mut server).await,
            json!({ "realtimeInput": { "activityEnd": {} } }),
            "the second start was de-duplicated, so the next frame is the close"
        );
    }

    /// The recovery constraint: `commit_audio` had a caller contract before it
    /// meant anything about activity — flush the pending buffer as a real
    /// `realtimeInput.audio` frame. Adding the manual-mode boundary must not
    /// change what an automatic-mode caller gets.
    #[tokio::test]
    async fn commit_audio_still_flushes_buffered_audio_under_server_detection() {
        let (session, mut server) = session_on_loopback(ActivityDetection::Automatic).await;

        let chunk = AudioChunk::from_i16_samples(&[0i16; 160], AudioFormat::pcm16_16khz());
        session.send_audio(&chunk).await.unwrap();
        session.commit_audio().await.unwrap();

        let frame = next_frame(&mut server).await;
        assert!(
            frame["realtimeInput"].get("audio").is_some(),
            "the buffered audio was not flushed: {frame}"
        );
    }

    /// The generic manual-turn boundary reaches Gemini as `activityEnd` after
    /// the queued audio has been flushed. Callers using `RealtimeRunner` do not
    /// need a downcast to the Gemini session to end a locally detected turn.
    #[tokio::test]
    async fn commit_audio_emits_activity_end_in_manual_mode() {
        let (session, mut server) = session_on_loopback(ActivityDetection::Manual).await;

        session.interrupt().await.unwrap();
        session.commit_audio().await.unwrap();

        assert_eq!(
            next_frame(&mut server).await,
            json!({ "realtimeInput": { "activityStart": {} } })
        );
        assert_eq!(
            next_frame(&mut server).await,
            json!({ "realtimeInput": { "activityEnd": {} } })
        );
    }

    /// A second local endpoint report is not a second turn boundary, and the
    /// end frame re-arms the next start through the same session state machine.
    #[tokio::test]
    async fn repeated_commit_audio_is_redundant_and_rearms_manual_turns() {
        let (session, mut server) = session_on_loopback(ActivityDetection::Manual).await;

        session.interrupt().await.unwrap();
        session.commit_audio().await.unwrap();
        session.commit_audio().await.unwrap();
        session.interrupt().await.unwrap();

        assert_eq!(
            next_frame(&mut server).await,
            json!({ "realtimeInput": { "activityStart": {} } })
        );
        assert_eq!(
            next_frame(&mut server).await,
            json!({ "realtimeInput": { "activityEnd": {} } })
        );
        assert_eq!(
            next_frame(&mut server).await,
            json!({ "realtimeInput": { "activityStart": {} } })
        );
    }

    /// In the default server-owned mode a commit ends the audio stream so the
    /// server closes the turn; it must not emit an `activityEnd` the protocol
    /// forbids in this mode.
    #[tokio::test]
    async fn commit_audio_writes_no_activity_end_when_server_detection_is_on() {
        let (session, mut server) = session_on_loopback(ActivityDetection::Automatic).await;

        session.commit_audio().await.unwrap();
        session.send_text("sentinel").await.unwrap();

        assert_eq!(
            next_frame(&mut server).await,
            json!({ "realtimeInput": { "audioStreamEnd": true } }),
            "commit must end the audio stream rather than send an activity frame"
        );
        let frame = next_frame(&mut server).await;
        assert_eq!(
            frame["realtimeInput"]["text"], "sentinel",
            "an activity frame preceded the sentinel: {frame}"
        );
    }

    /// One barge-in reported by several observers is still one transition.
    /// A second `activityStart` would be the duplicate-cancel failure RFC 6787
    /// §6.2.4 describes.
    #[tokio::test]
    async fn one_barge_in_produces_one_activity_start() {
        let (session, mut server) = session_on_loopback(ActivityDetection::Manual).await;
        let activity = session.activity_signaller().expect("manual mode offers a signaller");

        assert_eq!(activity.start().await.unwrap(), ActivitySignalOutcome::Sent);
        assert_eq!(activity.start().await.unwrap(), ActivitySignalOutcome::Redundant);
        // `interrupt` shares the state machine, so it does not re-signal either.
        session.interrupt().await.unwrap();

        assert_eq!(
            next_frame(&mut server).await,
            json!({ "realtimeInput": { "activityStart": {} } })
        );

        // Only the sentinel should follow the single start frame.
        session.send_text("sentinel").await.unwrap();
        let frame = next_frame(&mut server).await;
        assert_eq!(
            frame["realtimeInput"]["text"], "sentinel",
            "a duplicate activity frame was sent: {frame}"
        );
    }

    /// Ending the turn has to re-arm the next one, or a session signals once
    /// and then goes permanently deaf.
    #[tokio::test]
    async fn ending_activity_re_arms_the_next_start() {
        let (session, mut server) = session_on_loopback(ActivityDetection::Manual).await;
        let activity = session.activity_signaller().unwrap();

        assert_eq!(activity.start().await.unwrap(), ActivitySignalOutcome::Sent);
        assert_eq!(activity.end().await.unwrap(), ActivitySignalOutcome::Sent);
        assert_eq!(activity.end().await.unwrap(), ActivitySignalOutcome::Redundant);
        assert_eq!(activity.start().await.unwrap(), ActivitySignalOutcome::Sent);

        assert_eq!(
            next_frame(&mut server).await,
            json!({ "realtimeInput": { "activityStart": {} } })
        );
        assert_eq!(
            next_frame(&mut server).await,
            json!({ "realtimeInput": { "activityEnd": {} } })
        );
        assert_eq!(
            next_frame(&mut server).await,
            json!({ "realtimeInput": { "activityStart": {} } })
        );
    }

    /// `ResponseCancel` is actionable once the client owns detection, so it
    /// should stop being dropped with a warning.
    #[tokio::test]
    async fn response_cancel_becomes_a_real_cancel_in_manual_mode() {
        let (session, mut server) = session_on_loopback(ActivityDetection::Manual).await;

        session.send_event(ClientEvent::ResponseCancel).await.unwrap();

        assert_eq!(
            next_frame(&mut server).await,
            json!({ "realtimeInput": { "activityStart": {} } })
        );
    }

    fn tool_with_constrained_schema() -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "submit".to_string(),
            description: Some("a tool".to_string()),
            parameters: Some(json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {"n": {"type": "string", "minLength": 7}}
            })),
            behavior: None,
            effect: adk_core::ToolEffect::NonIdempotent,
        }]
    }

    /// The default stays on the legacy field, so nothing changes for callers
    /// who do not opt in.
    #[test]
    fn tools_default_to_the_legacy_parameters_field() {
        let cache = adk_core::SchemaCache::new();
        let adapter = adk_gemini::schema_adapter::GeminiSchemaAdapter::new();
        let caps = capabilities_for("models/gemini-3.1-flash-live-preview");
        let tools = convert_tools(Some(tool_with_constrained_schema()), &cache, &adapter, caps)
            .unwrap()
            .unwrap();
        let decl = &tools[0]["functionDeclarations"][0];

        assert!(decl.get("parameters").is_some(), "{decl}");
        assert!(decl.get("parametersJsonSchema").is_none(), "{decl}");
    }

    /// The bug this exists to fix: a schema that kept `additionalProperties`
    /// must not be posted under `parameters`. Gemini answers that frame by
    /// closing the socket with WS 1007, so the call dies before any audio
    /// flows — the two fields are mutually exclusive, never interchangeable.
    #[test]
    fn json_schema_dialect_posts_under_parameters_json_schema() {
        let cache = adk_core::SchemaCache::new();
        let adapter = adk_gemini::schema_adapter::GeminiSchemaAdapter::json_schema();
        let caps = capabilities_for("models/gemini-3.1-flash-live-preview");
        let tools = convert_tools(Some(tool_with_constrained_schema()), &cache, &adapter, caps)
            .unwrap()
            .unwrap();
        let decl = &tools[0]["functionDeclarations"][0];

        assert!(decl.get("parameters").is_none(), "both fields sent: {decl}");
        assert_eq!(decl["parametersJsonSchema"]["additionalProperties"], json!(false));
        assert_eq!(decl["parametersJsonSchema"]["properties"]["n"]["minLength"], 7);
    }

    /// The dialect held on the session — not a default — is what the setup
    /// frame carries. `build_setup_message` takes the dialect as an argument,
    /// so nothing but this test observes whether `send_setup` passes its own.
    /// Getting it wrong sends constraints under `parameters` and Gemini Live
    /// closes the socket with WS 1007.
    #[tokio::test]
    async fn send_setup_posts_tools_under_the_session_dialect() {
        let (session, mut server) = session_on_loopback_with_dialect(
            ActivityDetection::Automatic,
            adk_gemini::GeminiSchemaDialect::JsonSchema,
        )
        .await;

        let config =
            RealtimeConfig { tools: Some(tool_with_constrained_schema()), ..Default::default() };
        // Built from the session's own `dialect`, which is the thing under test:
        // rebuilding here proves `send_setup` used the session's dialect just as
        // reading a cached adapter off the session did, without the session
        // having to carry schema state its production paths never read.
        let adapter =
            adk_gemini::schema_adapter::GeminiSchemaAdapter::with_dialect(session.dialect);
        let caps = capabilities_for(&session.reconnect_model);
        let tools =
            convert_tools(config.tools.clone(), &adk_core::SchemaCache::new(), &adapter, caps)
                .unwrap();
        session.send_setup_with_compiled_tools("models/test", config, tools, None).await.unwrap();

        let frame = next_frame(&mut server).await;
        let decl = &frame["setup"]["tools"][0]["functionDeclarations"][0];

        assert!(decl.get("parameters").is_none(), "legacy field used: {frame}");
        assert_eq!(decl["parametersJsonSchema"]["additionalProperties"], json!(false));
    }

    /// AI Studio closes the socket with 1007 ("unexpected model name format") on a bare model
    /// id, so the setup names the model `models/{id}`; a name with a collection is sent as given.
    #[test]
    fn a_bare_studio_model_id_is_sent_as_its_resource_name() {
        let studio = GeminiLiveBackend::studio("key");
        for (given, sent) in [
            ("gemini-3.1-flash-live-preview", "models/gemini-3.1-flash-live-preview"),
            ("models/gemini-3.1-flash-live-preview", "models/gemini-3.1-flash-live-preview"),
            ("tunedModels/live-1", "tunedModels/live-1"),
        ] {
            let model_name = live_model_name(&studio, given);
            let msg = GeminiRealtimeSession::build_setup_message(
                &model_name,
                RealtimeConfig::default(),
                None,
                None,
            )
            .unwrap();
            let frame = serde_json::to_value(&msg).unwrap();
            assert_eq!(frame["setup"]["model"], json!(sent), "{given}");
        }
    }

    /// Vertex AI names publisher models by its own resource path, so its ids are not rewritten.
    #[cfg(feature = "vertex-live")]
    #[test]
    fn vertex_model_names_pass_unchanged() {
        let vertex = GeminiLiveBackend::Vertex {
            credentials: google_cloud_auth::credentials::anonymous::Builder::new().build(),
            region: "us-central1".to_string(),
            project_id: "project".to_string(),
            endpoint_url: None,
        };
        for name in [
            "gemini-3.1-flash-live-preview",
            "projects/project/locations/us-central1/publishers/google/models/gemini-live",
        ] {
            assert_eq!(live_model_name(&vertex, name), name);
        }
    }

    /// Frames recorded from Gemini Live (`gemini-3.1-flash-live-preview`): the user starts
    /// speaking while the model answers, the server interrupts the model, transcribes the
    /// user, and reports the end of the speech. The activity reaches callers as the speech
    /// events OpenAI sends, with the offset into the audio stream.
    #[tokio::test]
    async fn voice_activity_is_reported_as_speech_started_and_stopped() {
        let (session, mut server) = session_on_loopback(ActivityDetection::Automatic).await;
        for frame in [
            r#"{"serverContent":{"speechState":"SPEECH"},"voiceActivity":{"type":"ACTIVITY_START","audioOffset":"7.280s"}}"#,
            r#"{"serverContent":{"interrupted":true}}"#,
            r#"{"serverContent":{"turnComplete":true},"usageMetadata":{"totalTokenCount":579}}"#,
            r#"{"serverContent":{"inputTranscription":{"text":"Stop, please. What is 2 + 2?"}}}"#,
            r#"{"serverContent":{"speechState":"NON_SPEECH"},"voiceActivity":{"type":"ACTIVITY_END","audioOffset":"10.640s"}}"#,
        ] {
            server.send(Message::Text(frame.into())).await.unwrap();
        }

        let mut events = Vec::new();
        while events.len() < 6 {
            let event =
                tokio::time::timeout(std::time::Duration::from_secs(5), session.next_event())
                    .await
                    .expect("timed out waiting for an event")
                    .expect("session ended")
                    .expect("event");
            if !matches!(event, ServerEvent::Unknown) {
                events.push(event);
            }
        }

        // The interrupted frame cancels the abandoned generation (the onset was
        // already reported by `voiceActivity`, so no second `SpeechStarted`),
        // and the turn-final counts surface as a `Usage` event beside the
        // `ResponseDone` that carries them.
        assert!(
            matches!(
                events.as_slice(),
                [
                    ServerEvent::SpeechStarted { audio_start_ms: 7280, .. },
                    ServerEvent::ResponseCancelled { .. },
                    ServerEvent::ResponseDone { .. },
                    ServerEvent::Usage { .. },
                    ServerEvent::InputTranscriptDelta { delta, .. },
                    ServerEvent::SpeechStopped { audio_end_ms: 10640, .. },
                ] if delta == "Stop, please. What is 2 + 2?"
            ),
            "{events:?}"
        );
    }

    /// A message that carries `voiceActivity` beside content keeps both, activity first; an
    /// unspecified activity is not a speech event, and a missing offset reads as zero.
    #[tokio::test]
    async fn voice_activity_beside_content_keeps_both() {
        let (session, _server) = session_on_loopback(ActivityDetection::Automatic).await;
        let events = session
            .translate_gemini_event(
                r#"{"serverContent":{"inputTranscription":{"text":"hi"}},"voiceActivity":{"type":"ACTIVITY_END","audioOffset":"1.5s"}}"#,
            )
            .unwrap();
        assert!(
            matches!(
                events.as_slice(),
                [
                    ServerEvent::SpeechStopped { audio_end_ms: 1500, .. },
                    ServerEvent::InputTranscriptDelta { delta, .. },
                ] if delta == "hi"
            ),
            "{events:?}"
        );

        for frame in [
            r#"{"voiceActivity":{"type":"TYPE_UNSPECIFIED","audioOffset":"1s"}}"#,
            r#"{"voiceActivity":{"audioOffset":"1s"}}"#,
        ] {
            let events = session.translate_gemini_event(frame).unwrap();
            assert!(matches!(events.as_slice(), [ServerEvent::Unknown]), "{frame}: {events:?}");
        }
        let events = session
            .translate_gemini_event(r#"{"voiceActivity":{"type":"ACTIVITY_START"}}"#)
            .unwrap();
        assert!(
            matches!(events.as_slice(), [ServerEvent::SpeechStarted { audio_start_ms: 0, .. }]),
            "{events:?}"
        );
    }

    // ── Recorded frames: resumption handles and interruptions ────────────

    /// Gemini Live sends the resumption handle as `newHandle` (frames recorded from
    /// `gemini-3.1-flash-live-preview`, handle replaced). The handle is a
    /// provider-private credential: it is latched on the session for the next
    /// reconnect and never surfaces as a public event. While the session cannot
    /// be resumed the update carries no usable handle and the last one stands.
    #[tokio::test]
    async fn the_resumption_handle_is_read_from_new_handle() {
        let (session, _server) = session_on_loopback(ActivityDetection::Automatic).await;
        let handle = |frame: &str| {
            let events = session.translate_gemini_event(frame).unwrap();
            assert!(events.is_empty(), "resumption updates emit no event: {events:?}");
            session.last_resume_handle()
        };

        assert_eq!(
            handle(
                r#"{"sessionResumptionUpdate":{"newHandle":"4c2b7e0a-9f1d-4e55-8a6b-1d2e3f405162","resumable":true}}"#
            )
            .as_deref(),
            Some("4c2b7e0a-9f1d-4e55-8a6b-1d2e3f405162")
        );
        assert_eq!(
            handle("{\n  \"sessionResumptionUpdate\": {\n    \"newHandle\": \"h-2\",\n    \"resumable\": true\n  }\n}\n")
                .as_deref(),
            Some("h-2")
        );
        // `resumptionToken` is not a field the server sends; it is not read.
        assert_eq!(
            handle(r#"{"sessionResumptionUpdate":{"resumptionToken":"h-legacy"}}"#).as_deref(),
            Some("h-2")
        );
        assert_eq!(
            handle(r#"{"sessionResumptionUpdate":{"resumable":false}}"#).as_deref(),
            Some("h-2")
        );
        assert_eq!(
            handle(r#"{"sessionResumptionUpdate":{"newHandle":"","resumable":false}}"#).as_deref(),
            Some("h-2")
        );
        // An empty handle is no handle, even on a resumable update.
        assert_eq!(
            handle(r#"{"sessionResumptionUpdate":{"newHandle":"","resumable":true}}"#).as_deref(),
            Some("h-2")
        );
        assert_eq!(
            handle(r#"{"sessionResumptionUpdate":{"newHandle":"   ","resumable":true}}"#)
                .as_deref(),
            Some("h-2")
        );
    }

    /// A latched handle older than its TTL is dropped, not presented.
    #[tokio::test]
    async fn an_expired_resumption_handle_is_dropped() {
        let (session, _server) = session_on_loopback(ActivityDetection::Automatic).await;
        *session.last_resume_handle.lock() = Some((
            "stale-handle".to_string(),
            std::time::Instant::now() - (RESUME_HANDLE_TTL + std::time::Duration::from_secs(1)),
        ));
        assert_eq!(session.fresh_resume_handle(), None);
        assert_eq!(session.last_resume_handle(), None, "the expired handle is dropped");
    }

    /// `serverContent.interrupted` (recorded) is the server's report that the user's speech
    /// cut the model off. It reaches callers as `SpeechStarted` (the barge-in onset, unless
    /// `voiceActivity` already reported it) followed by `ResponseCancelled` (the abandoned
    /// generation the runner must purge); a speech whose onset `voiceActivity` already
    /// reported is reported once.
    #[tokio::test]
    async fn interrupted_is_reported_as_speech_started() {
        let (session, _server) = session_on_loopback(ActivityDetection::Automatic).await;
        let interrupted = "{\n  \"serverContent\": {\n    \"interrupted\": true\n  }\n}\n";

        let events = session.translate_gemini_event(interrupted).unwrap();
        assert!(
            matches!(
                events.as_slice(),
                [
                    ServerEvent::SpeechStarted { audio_start_ms: 0, .. },
                    ServerEvent::ResponseCancelled { .. },
                ]
            ),
            "{events:?}"
        );

        let onset = session
            .translate_gemini_event(
                r#"{"serverContent":{"speechState":"SPEECH"},"voiceActivity":{"type":"ACTIVITY_START","audioOffset":"0.280s"}}"#,
            )
            .unwrap();
        assert!(
            matches!(onset.as_slice(), [ServerEvent::SpeechStarted { audio_start_ms: 280, .. }]),
            "{onset:?}"
        );
        // The onset is latched, so no second `SpeechStarted` — but the abandoned
        // generation still cancels.
        let events = session.translate_gemini_event(interrupted).unwrap();
        assert!(matches!(events.as_slice(), [ServerEvent::ResponseCancelled { .. }]), "{events:?}");

        session
            .translate_gemini_event(
                r#"{"serverContent":{"speechState":"NON_SPEECH"},"voiceActivity":{"type":"ACTIVITY_END","audioOffset":"3.880s"}}"#,
            )
            .unwrap();
        let events = session.translate_gemini_event(interrupted).unwrap();
        assert!(
            matches!(
                events.as_slice(),
                [ServerEvent::SpeechStarted { .. }, ServerEvent::ResponseCancelled { .. },]
            ),
            "{events:?}"
        );
    }

    /// A usage report on a message of its own belongs to the response that
    /// closes next: the session stashes it instead of emitting it, so the
    /// counts surface once, attached to the closing `ResponseDone`.
    #[tokio::test]
    async fn standalone_usage_attaches_to_the_response_that_closes_next() {
        let (session, _server) = session_on_loopback(ActivityDetection::Automatic).await;

        let stashed =
            session.translate_gemini_event(r#"{"usageMetadata":{"totalTokenCount":579}}"#).unwrap();
        assert!(matches!(stashed.as_slice(), [ServerEvent::Unknown]), "{stashed:?}");

        let closed =
            session.translate_gemini_event(r#"{"serverContent":{"turnComplete":true}}"#).unwrap();
        match closed.as_slice() {
            [ServerEvent::ResponseDone { response, .. }] => {
                assert_eq!(
                    response["usageMetadata"]["totalTokenCount"],
                    json!(579),
                    "{response:?}"
                );
            }
            other => panic!("expected ResponseDone carrying the stashed usage, got {other:?}"),
        }

        // Nothing left behind: the next bare close carries no counts.
        let bare =
            session.translate_gemini_event(r#"{"serverContent":{"turnComplete":true}}"#).unwrap();
        match bare.as_slice() {
            [ServerEvent::ResponseDone { response, .. }] => {
                assert!(response.get("usageMetadata").is_none(), "{response:?}");
            }
            other => panic!("expected a bare ResponseDone, got {other:?}"),
        }
    }

    // ── RealtimeAgent on Gemini Live ─────────────────────────────────────

    /// The close reason Gemini Live sends for a setup with two response modalities, recorded
    /// from `gemini-3.1-flash-live-preview` (the API cuts a close reason at 123 bytes).
    const MODALITIES_REFUSED: &str = "The requested combination of response modalities (AUDIO, TEXT) is not supported by the model. models/gemini-3.1-flash-live-";

    /// A model whose sessions talk to a loopback server that keeps Gemini Live's setup rule
    /// for `gemini-3.1-flash-live-preview` — one response modality, `AUDIO` — and closes with
    /// 1007 otherwise, as the API does. It answers the first turn with recorded frames and
    /// then closes the session normally.
    struct LoopbackGeminiLive;

    #[async_trait]
    impl crate::model::RealtimeModel for LoopbackGeminiLive {
        fn provider(&self) -> &str {
            "gemini"
        }

        fn model_id(&self) -> &str {
            "models/gemini-3.1-flash-live-preview"
        }

        fn supported_input_formats(&self) -> Vec<AudioFormat> {
            vec![AudioFormat::pcm16_16khz()]
        }

        fn supported_output_formats(&self) -> Vec<AudioFormat> {
            vec![AudioFormat::pcm16_24khz()]
        }

        fn available_voices(&self) -> Vec<&str> {
            vec!["Kore"]
        }

        async fn connect(&self, config: RealtimeConfig) -> Result<crate::session::BoxedSession> {
            let (session, server) =
                session_on_loopback(ActivityDetection::from_config(&config)).await;
            tokio::spawn(gemini_live_server(server));
            let adapter =
                adk_gemini::schema_adapter::GeminiSchemaAdapter::with_dialect(session.dialect);
            let caps = capabilities_for(&session.reconnect_model);
            let tools =
                convert_tools(config.tools.clone(), &adk_core::SchemaCache::new(), &adapter, caps)?;
            session.send_setup_with_compiled_tools(self.model_id(), config, tools, None).await?;
            Ok(Box::new(session))
        }
    }

    async fn gemini_live_server(mut server: ServerEnd) {
        use tokio_tungstenite::tungstenite::protocol::CloseFrame;
        use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

        let setup = next_frame(&mut server).await;
        let modalities = setup["setup"]["generationConfig"]["responseModalities"].as_array();
        let audio_only = matches!(
            modalities.map(Vec::as_slice),
            Some([only]) if only.as_str().is_some_and(|m| m.eq_ignore_ascii_case("audio"))
        );
        if !audio_only {
            let _ = server
                .close(Some(CloseFrame {
                    code: CloseCode::Invalid,
                    reason: MODALITIES_REFUSED.into(),
                }))
                .await;
            return;
        }
        server.send(Message::Text(r#"{"setupComplete":{}}"#.into())).await.unwrap();
        let turn = next_frame(&mut server).await;
        assert_eq!(
            turn["realtimeInput"]["text"], "What is two plus two?",
            "the text turn must arrive as realtimeInput text: {turn}"
        );
        for frame in [
            r#"{"serverContent":{"modelTurn":{"parts":[{"inlineData":{"mimeType":"audio/pcm;rate=24000","data":"AAABAAIA"}}]}}}"#,
            r#"{"serverContent":{"modelTurn":{"parts":[{"inlineData":{"mimeType":"audio/pcm;rate=24000","data":"AwAEAAUA"}}]}}}"#,
            r#"{"serverContent":{"generationComplete":true}}"#,
            r#"{"serverContent":{"turnComplete":true},"usageMetadata":{"promptTokenCount":155,"responseTokenCount":42,"totalTokenCount":197}}"#,
        ] {
            server.send(Message::Text(frame.into())).await.unwrap();
        }
        let _ = server.close(Some(CloseFrame { code: CloseCode::Normal, reason: "".into() })).await;
    }

    /// Runs `agent` once under `Runner` with a text turn; returns what the run yielded and
    /// what the session saved.
    async fn run_agent(
        agent: crate::RealtimeAgent,
    ) -> (Vec<adk_core::Result<adk_core::Event>>, Vec<adk_core::Event>) {
        use adk_session::{CreateRequest, GetRequest, InMemorySessionService, SessionService};

        let sessions = Arc::new(InMemorySessionService::new());
        sessions
            .create(CreateRequest {
                app_name: "voice-app".to_string(),
                user_id: "ana".to_string(),
                session_id: Some("s1".to_string()),
                state: Default::default(),
            })
            .await
            .unwrap();
        let runner = adk_runner::Runner::builder()
            .app_name("voice-app")
            .agent(Arc::new(agent))
            .session_service(sessions.clone() as Arc<dyn SessionService>)
            .build()
            .unwrap();
        let results: Vec<_> = runner
            .run_str("ana", "s1", adk_core::Content::new("user").with_text("What is two plus two?"))
            .await
            .unwrap()
            .collect()
            .await;
        let saved = sessions
            .get(GetRequest {
                app_name: "voice-app".to_string(),
                user_id: "ana".to_string(),
                session_id: "s1".to_string(),
                num_recent_events: None,
                after: None,
            })
            .await
            .unwrap()
            .events()
            .all();
        (results, saved)
    }

    /// A `RealtimeAgent` built with default settings leaves the modalities to the provider,
    /// so Gemini Live accepts its setup and the spoken reply arrives.
    #[tokio::test]
    async fn a_default_realtime_agent_runs_on_gemini_live() {
        let agent = crate::RealtimeAgent::builder("voice")
            .model(Arc::new(LoopbackGeminiLive))
            .build()
            .unwrap();
        let (results, saved) = run_agent(agent).await;

        assert!(results.iter().all(std::result::Result::is_ok), "{results:?}");
        let audio: Vec<u8> = saved
            .iter()
            .filter(|event| event.author == "voice")
            .filter_map(|event| event.llm_response.content.as_ref())
            .flat_map(|content| &content.parts)
            .filter_map(|part| match part {
                Part::InlineData { mime_type, data, .. } if mime_type == "audio/pcm" => {
                    Some(data.clone())
                }
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(audio, vec![0, 0, 1, 0, 2, 0, 3, 0, 4, 0, 5, 0]);
    }

    /// A setup Gemini Live refuses closes the socket with 1007; the run ends with that error
    /// instead of ending as though the session had finished.
    #[tokio::test]
    async fn a_refused_setup_ends_the_run_with_the_providers_reason() {
        let agent = crate::RealtimeAgent::builder("voice")
            .model(Arc::new(LoopbackGeminiLive))
            .modalities(vec!["text".to_string(), "audio".to_string()])
            .build()
            .unwrap();
        let (results, _) = run_agent(agent).await;

        let errors: Vec<_> = results.iter().filter_map(|result| result.as_ref().err()).collect();
        assert_eq!(errors.len(), 1, "{results:?}");
        assert_eq!(errors[0].code, "model.realtime.session_closed");
        assert_eq!(errors[0].category, adk_core::ErrorCategory::InvalidInput);
        assert!(errors[0].message.contains(&format!("1007:{MODALITIES_REFUSED}")), "{}", errors[0]);
    }
}
