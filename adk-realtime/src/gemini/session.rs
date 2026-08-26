//! Gemini Live session implementation.
//!
//! Manages a WebSocket connection to Google's Gemini Live API with support
//! for both AI Studio (API key) and Vertex AI (OAuth/ADC) backends.

use crate::audio::{AudioChunk, AudioFormat};
use crate::config::{RealtimeConfig, ToolDefinition};
use crate::error::{RealtimeError, Result};
use crate::events::{ClientEvent, ServerEvent, ToolResponse};
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

/// Backend configuration for Gemini Live connections.
///
/// Determines how to authenticate and which endpoint to connect to.
#[derive(Debug, Clone)]
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

#[derive(Debug, Clone, Serialize)]
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

/// Configuration for Gemini 2.5 Live session resumption.
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
    realtime_input_config: Option<Value>,
}

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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiRealtimeInput<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    audio: Option<GeminiMediaChunk<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    media_chunks: Option<Vec<GeminiMediaChunk<'a>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiTurn {
    role: String,
    parts: Vec<GeminiPart>,
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
    schema_cache: Arc<adk_core::SchemaCache>,
    adapter: Arc<dyn adk_core::SchemaAdapter>,
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
    /// The most recent resumable handle received via `sessionResumptionUpdate`.
    ///
    /// Gemini Live emits a new handle roughly every 60 s when `resumable: true`.
    /// Only resumable handles are stored here; a non-resumable update must not
    /// overwrite the last safe point (the previous resumable handle remains the
    /// best reconnect anchor).
    last_resume_handle: Arc<ParkingMutex<Option<String>>>,
    /// Association map between function call IDs and tool names for wire-conforming `FunctionResponse` payloads.
    call_names: Arc<ParkingMutex<CallNamesMap>>,
    /// Watch channel broadcasting replacement deadlines announced by Gemini Live `goAway` frames.
    replacement_deadline_tx: tokio::sync::watch::Sender<Option<std::time::Instant>>,
    replacement_deadline_rx: tokio::sync::watch::Receiver<Option<std::time::Instant>>,
}

const MAX_CALL_NAMES_CAPACITY: usize = 1000;

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
        self.last_resume_handle.lock().clone()
    }

    #[cfg(not(any(test, feature = "recovery-test-utils")))]
    pub(crate) fn last_resume_handle(&self) -> Option<String> {
        self.last_resume_handle.lock().clone()
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
            schema_cache: Arc::new(adk_core::SchemaCache::new()),
            adapter: Arc::new(adk_core::GenericSchemaAdapter),
            frame_log: FrameLog::new(session_id, "studio", reconnect_model.clone()),
            last_disconnect: Arc::new(ParkingMutex::new(None)),
            backend,
            dialect,
            reconnect_model,
            last_resume_handle: Arc::new(ParkingMutex::new(None)),
            call_names: Arc::new(ParkingMutex::new(CallNamesMap::new())),
            replacement_deadline_tx,
            replacement_deadline_rx,
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
        let normalized_model = normalize_model_id(model);
        let schema_cache = Arc::new(adk_core::SchemaCache::new());
        let adapter: Arc<dyn adk_core::SchemaAdapter> =
            Arc::new(adk_gemini::schema_adapter::GeminiSchemaAdapter::with_dialect(dialect));

        // 1. Compile tools BEFORE establishing the WebSocket connection.
        // If any tool fails to compile (semantic loss), we abort early.
        let tools = convert_tools(config.tools.clone(), &schema_cache, adapter.as_ref())?;

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
            schema_cache,
            adapter,
            frame_log,
            backend: backend.clone(),
            dialect,
            reconnect_model: normalized_model.clone(),
            last_resume_handle: Arc::new(ParkingMutex::new(private_resume_handle.clone())),
            call_names: Arc::new(ParkingMutex::new(CallNamesMap::new())),
            replacement_deadline_tx,
            replacement_deadline_rx,
        };

        session
            .send_setup_with_compiled_tools(&normalized_model, config, tools, private_resume_handle)
            .await?;
        Ok(session)
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
        let mut generation_config = json!({
            "responseModalities": gemini_response_modalities(config.modalities.as_deref()),
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

        if let Some(temp) = config.temperature {
            generation_config["temperature"] = json!(temp);
        }

        // Emotion-aware ("affective") dialog — a generationConfig field, honored
        // by native-audio models on the v1alpha endpoint.
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

        // Session resumption handle is strictly supplied by the Gemini adapter itself.
        let session_resumption = Some(SessionResumptionConfig { handle: private_resume_handle });

        // When transcription is requested, enable both input (user speech) and
        // output (model speech) transcription so consumers get clean text for
        // native-audio turns. An empty object turns the feature on.
        let transcription = config.input_audio_transcription.as_ref().map(|_| json!({}));

        let setup = GeminiClientMessage {
            setup: Some(GeminiSetup {
                model: Some(normalized_model.clone()),
                system_instruction,
                generation_config: Some(generation_config),
                tools: compiled_tools,
                cached_content: config.cached_content,
                session_resumption,
                input_audio_transcription: transcription.clone(),
                realtime_input_config,
                output_audio_transcription: transcription,
            }),
            realtime_input: None,
            tool_response: None,
            client_content: None,
        };

        tracing::info!(model_id = %normalized_model, "Sending Gemini Live setup");
        self.send_raw(&setup).await
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
    /// Also intercepts `SessionUpdated` events carrying a resumable handle and
    /// caches the handle on `self.last_resume_handle` so that
    /// `reconnect_with_backoff` can present it on the next connection attempt.
    pub(crate) fn translate_gemini_event(&self, raw: &str) -> Result<Vec<ServerEvent>> {
        if let Ok(value) = serde_json::from_str::<Value>(raw) {
            if let Some(resumption_update) = value.get("sessionResumptionUpdate") {
                let resumable =
                    resumption_update.get("resumable").and_then(Value::as_bool).unwrap_or(false);
                if resumable
                    && let Some(handle) = resumption_update.get("newHandle").and_then(Value::as_str)
                {
                    *self.last_resume_handle.lock() = Some(handle.to_owned());
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

        let events = Self::translate_event_logged(raw, Some(&self.frame_log))?;
        for event in &events {
            match event {
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

    /// Test entry point: no session, so no per-connection telemetry identity.
    pub fn translate_event_static(raw: &str) -> Result<Vec<ServerEvent>> {
        Self::translate_event_logged(raw, None)
    }

    pub(crate) fn translate_event_logged(
        raw: &str,
        frame_log: Option<&FrameLog>,
    ) -> Result<Vec<ServerEvent>> {
        let value: Value = serde_json::from_str(raw)
            .map_err(|e| RealtimeError::protocol(describe_unparseable_frame(raw, &e)))?;
        log_frame_shape(&value, frame_log);

        // Check for setup completion
        if let Some(_setup_complete) = value.get("setupComplete") {
            return Ok(vec![ServerEvent::SessionCreated {
                event_id: uuid::Uuid::new_v4().to_string(),
                session: value.clone(),
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
                events.push(ServerEvent::ResponseDone {
                    event_id: uuid::Uuid::new_v4().to_string(),
                    response: value.clone(),
                });
            }

            if !events.is_empty() {
                return Ok(events);
            }
        }

        // `sessionResumptionUpdate` carries `newHandle` and `resumable`. This
        // read `resumptionToken`, under a comment asserting a protocol asymmetry
        // — the server does not send that field, so the branch never fired and
        // no handle was ever captured. Resumption looked supported and was not.
        //
        // `resumable` is not decoration: Google specifies a handle is usable
        // only when it is true. An update with `resumable: false` means "there
        // is no safe resume point right now", so it must not overwrite the last
        // handle that *was* safe — that would trade a working resume point for
        // an unusable one.
        //
        // Capturing the handle is not the same as being able to reconnect with
        // it. Nothing here reconnects yet, so session resumption remains
        // unsupported; this is the first of the two halves.
        if let Some(resumption_update) = value.get("sessionResumptionUpdate") {
            let resumable =
                resumption_update.get("resumable").and_then(|r| r.as_bool()).unwrap_or(false);
            let handle = resumption_update.get("newHandle").and_then(|h| h.as_str());
            match (resumable, handle) {
                (true, Some(_handle)) => {
                    if let Some(frame_log) = frame_log {
                        let _ = frame_log; // suppress unused warning in non-session context
                    }
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
        let prior_handle = self.last_resume_handle();

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

        let attempt_fut = async {
            // Construct private candidate session passing prior_handle directly to provider setup
            let candidate_session = Self::connect_with_dialect_and_resume(
                self.backend.clone(),
                &self.reconnect_model,
                config.clone(),
                self.dialect,
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
            if candidate.last_resume_handle().is_none() && prior_handle.is_some() {
                *candidate.last_resume_handle.lock() = prior_handle;
            }

            let recovered: Arc<dyn RealtimeSession> = candidate;
            Ok(RecoveredSession::new(recovered, RecoveryContinuity::Reconnected))
        };

        match tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), attempt_fut).await {
            Ok(res) => res,
            Err(_) => Err(RealtimeError::Timeout(
                "Gemini recovery attempt timed out waiting for setupComplete".to_string(),
            )),
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
            setup: None,
            realtime_input: Some(GeminiRealtimeInput {
                audio: Some(GeminiMediaChunk {
                    // Gemini Live requires raw 16-bit PCM little-endian at 16 kHz;
                    // declaring the rate removes any server-side ambiguity.
                    mime_type: "audio/pcm;rate=16000",
                    data: audio_base64,
                }),
                media_chunks: None,
                text: None,
            }),
            tool_response: None,
            client_content: None,
        };
        self.send_raw(&msg).await
    }

    async fn send_video_frame(&self, mime_type: &str, data_base64: &str) -> Result<()> {
        // Gemini Live accepts image frames as realtimeInput media chunks.
        let msg = GeminiClientMessage {
            setup: None,
            realtime_input: Some(GeminiRealtimeInput {
                audio: None,
                media_chunks: Some(vec![GeminiMediaChunk { mime_type, data: data_base64 }]),
                text: None,
            }),
            tool_response: None,
            client_content: None,
        };
        self.send_raw(&msg).await
    }

    async fn send_text(&self, text: &str) -> Result<()> {
        // Send text via realtimeInput as required by Gemini 3.1 Live API for conversational text turns.
        let msg = GeminiClientMessage {
            setup: None,
            realtime_input: Some(GeminiRealtimeInput {
                audio: None,
                media_chunks: None,
                text: Some(text.to_string()),
            }),
            tool_response: None,
            client_content: None,
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
            setup: None,
            realtime_input: None,
            tool_response: Some(GeminiToolResponse {
                function_responses: vec![GeminiFunctionResponse {
                    id: response.call_id,
                    name,
                    response: output,
                }],
            }),
            client_content: None,
        };
        self.send_raw(&msg).await
    }

    async fn commit_audio(&self) -> Result<()> {
        self.flush_audio().await
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
        // Strategic flush: clear any buffered audio that hasn't been sent
        self.clear_audio().await?;
        Ok(()) // Gemini handles interruption via VAD
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
            ClientEvent::ResponseCancel => {
                tracing::warn!(
                    "Gemini Live API natively handles interruption via VAD. Manual ResponseCancel is unsupported. Dropping event."
                );
                Ok(())
            }
            ClientEvent::SessionUpdate { session } => {
                tracing::info!("Translating ClientEvent::SessionUpdate into Gemini Setup frame");
                let config: RealtimeConfig = serde_json::from_value(session).map_err(|e| {
                    RealtimeError::protocol(format!("Failed to parse SessionUpdate config: {e}"))
                })?;

                let mut generation_config = json!({});
                if config.modalities.is_some() {
                    generation_config["responseModalities"] =
                        json!(gemini_response_modalities(config.modalities.as_deref()));
                }

                if let Some(voice) = &config.voice {
                    generation_config["speechConfig"] = json!({
                        "voiceConfig": {
                            "prebuiltVoiceConfig": {
                                "voiceName": voice
                            }
                        }
                    });
                }

                if let Some(temp) = config.temperature {
                    generation_config["temperature"] = json!(temp);
                }

                // Computed before anything moves out of `config`. Session
                // updates rebuild setup from the live config, so the VAD
                // projection has to come along or a mid-call update would
                // silently reset it to Google's defaults.
                let realtime_input_config = gemini_realtime_input_config(&config);
                let system_instruction = config.instruction.map(|text| GeminiContent {
                    parts: vec![GeminiPart { text: Some(text), inline_data: None }],
                });

                // RE-USE the same retained compiler target for updates/resumption
                let tools = convert_tools(config.tools, &self.schema_cache, self.adapter.as_ref())?;

                let handle = self.last_resume_handle();
                let session_resumption = Some(SessionResumptionConfig { handle });

                let setup = GeminiClientMessage {
                    setup: Some(GeminiSetup {
                        model: config.model,
                        system_instruction,
                        generation_config: if generation_config.as_object().unwrap().is_empty() {
                            None
                        } else {
                            Some(generation_config)
                        },
                        tools,
                        cached_content: config.cached_content,
                        session_resumption,
                        input_audio_transcription: None,
                        output_audio_transcription: None,
                        realtime_input_config,
                    }),
                    realtime_input: None,
                    tool_response: None,
                    client_content: None,
                };

                self.send_raw(&setup).await
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
fn gemini_realtime_input_config(config: &RealtimeConfig) -> Option<Value> {
    let vad = config.turn_detection.as_ref()?;
    let mut detection = serde_json::Map::new();

    if vad.mode == crate::config::VadMode::None {
        // Automatic detection off means the caller drives turns explicitly.
        detection.insert("disabled".to_string(), json!(true));
    }

    // No rule needed about which values to trust: `VadConfig`'s defaults are
    // `None`, so a `Some` here is a caller's decision by construction.
    if let Some(ms) = vad.silence_duration_ms {
        detection.insert("silenceDurationMs".to_string(), json!(ms));
    }
    if let Some(ms) = vad.prefix_padding_ms {
        detection.insert("prefixPaddingMs".to_string(), json!(ms));
    }

    if vad.threshold.is_some() {
        tracing::warn!(
            "VadConfig.threshold is not representable on Gemini Live, which exposes only \
             coarse sensitivity levels; the configured value is not applied"
        );
    }
    if vad.eagerness.is_some() {
        tracing::debug!("VadConfig.eagerness is OpenAI-specific and does not apply to Gemini");
    }

    let mut input_config = serde_json::Map::new();
    if !detection.is_empty() {
        input_config.insert("automaticActivityDetection".to_string(), Value::Object(detection));
    }
    if let Some(interrupt) = vad.interrupt_response {
        input_config.insert(
            "activityHandling".to_string(),
            json!(if interrupt { "START_OF_ACTIVITY_INTERRUPTS" } else { "NO_INTERRUPTION" }),
        );
    }

    (!input_config.is_empty()).then_some(Value::Object(input_config))
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
        setup: None,
        realtime_input: None,
        tool_response: None,
        client_content: Some(GeminiClientContent {
            turns: vec![GeminiTurn { role: gemini_role, parts: final_parts }],
            turn_complete: true,
        }),
    }
}

/// Convert ADK tool definitions to Gemini format.
fn convert_tools(
    tools: Option<Vec<ToolDefinition>>,
    cache: &adk_core::SchemaCache,
    adapter: &dyn adk_core::SchemaAdapter,
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
        if let Some(desc) = &t.description {
            decl["description"] = json!(desc);
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

    mod tool_declarations {
        use super::*;

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
            let tools = convert_tools(Some(vec![intake_tool()]), &cache, &adapter)
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
            assert!(
                subset["parameters"]["properties"]["callback_number"].get("minLength").is_none()
            );
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

            let tools = convert_tools(Some(vec![ToolDefinition::new("hangup")]), &cache, &adapter)
                .expect("a parameterless tool compiles")
                .expect("tools were supplied");
            let declaration = &tools[0]["functionDeclarations"][0];

            assert_eq!(
                declaration["parametersJsonSchema"],
                json!({"type": "object", "properties": {}})
            );
            assert!(declaration.get("parameters").is_none(), "{declaration}");
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
        // Conversational text turns must serialize via realtimeInput.text for Gemini 3.1 Live API
        let msg = GeminiClientMessage {
            setup: None,
            realtime_input: Some(GeminiRealtimeInput {
                audio: None,
                media_chunks: None,
                text: Some("Hello 3.1".to_string()),
            }),
            tool_response: None,
            client_content: None,
        };

        let js = serde_json::to_value(&msg).unwrap();
        assert!(js.get("clientContent").is_none());
        assert_eq!(
            js.get("realtimeInput").and_then(|r| r.get("text")).and_then(|t| t.as_str()),
            Some("Hello 3.1")
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
            realtime_input_config: None,
            output_audio_transcription: None,
        };
        let wrapper = GeminiClientMessage {
            setup: Some(setup),
            realtime_input: None,
            tool_response: None,
            client_content: None,
        };
        let js = serde_json::to_value(&wrapper).unwrap();
        let setup_json = js.get("setup").expect("setup missing").as_object().unwrap();
        assert_eq!(
            setup_json.get("model").expect("model missing from setup payload").as_str().unwrap(),
            "models/gemini-2.5-flash-native-audio-latest"
        );
    }

    #[test]
    fn test_affective_dialog_builder_sets_config() {
        let c = RealtimeConfig::default().with_affective_dialog(true);
        assert_eq!(c.affective_dialog, Some(true));
        assert_eq!(RealtimeConfig::default().affective_dialog, None);
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
    async fn test_connect_fail_closed_on_invalid_schema() {
        let backend = GeminiLiveBackend::studio("fake-key");
        let config = RealtimeConfig {
            tools: Some(vec![ToolDefinition {
                name: "invalid_tool".to_string(),
                description: Some("description".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "polymorphic": {
                            "anyOf": [{"type": "string"}, {"type": "number"}]
                        }
                    }
                })),
            }]),
            ..Default::default()
        };

        let result = GeminiRealtimeSession::connect(backend, "models/gemini-live", config).await;

        match result {
            Err(RealtimeError::Protocol(msg)) => {
                assert!(msg.contains("Failed to compile schema"));
                assert!(msg.contains("polymorphic unions"));
            }
            other => panic!(
                "Expected Protocol Error (via RealtimeError::protocol) due to schema compilation failure, got {:?}",
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
        });
        let projected = gemini_realtime_input_config(&config).expect("projection");
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
        });
        let projected = gemini_realtime_input_config(&config).expect("projection");
        let detection = projected.get("automaticActivityDetection").expect("detection block");

        assert_eq!(detection.get("disabled").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(detection.get("silenceDurationMs").and_then(|v| v.as_u64()), Some(700));
        assert_eq!(detection.get("prefixPaddingMs").and_then(|v| v.as_u64()), Some(300));
        assert_eq!(
            projected.get("activityHandling").and_then(|v| v.as_str()),
            Some("NO_INTERRUPTION")
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
        };

        let err = session1.send_tool_response(tool_response).await.unwrap_err();
        assert!(err.to_string().contains("Missing tool name for call_id 'call_gen0_789'"));

        // Verify zero messages reached Session 1's outbound tx queue
        assert!(
            rx1.try_recv().is_err(),
            "Zero tool response messages must reach Session 1's write queue"
        );
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
            "models/gemini-3.1-flash-live-preview".to_string(),
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
            "models/gemini-3.1-flash-live-preview".to_string(),
            tx,
            writer_task,
            source,
        );

        // Simulate provider caching fresh handle H2 ("fresh_provider_h2")
        *session.last_resume_handle.lock() = Some("fresh_provider_h2".to_string());

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
}

#[cfg(test)]
mod teardown_tests {
    use super::*;
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
            schema_cache: Arc::new(adk_core::SchemaCache::new()),
            adapter: Arc::new(adk_core::GenericSchemaAdapter),
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
}
