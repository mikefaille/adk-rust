//! Gemini Live session implementation.
//!
//! Manages a WebSocket connection to Google's Gemini Live API with support
//! for both AI Studio (API key) and Vertex AI (OAuth/ADC) backends.

use crate::audio::{AudioChunk, AudioFormat};
use crate::config::{RealtimeConfig, ToolDefinition};
use crate::error::{RealtimeError, Result};
use crate::events::{ClientEvent, ServerEvent, ToolResponse};
use crate::session::{ContextMutationOutcome, RealtimeSession};
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
    Studio { api_key: String },

    /// Vertex AI with OAuth2/ADC authentication.
    #[cfg(feature = "vertex-live")]
    Vertex {
        /// Google Cloud credentials for OAuth2 token generation.
        credentials: google_cloud_auth::credentials::Credentials,
        /// Google Cloud region (e.g., "us-central1").
        region: String,
        /// Google Cloud project ID.
        project_id: String,
    },
}

impl GeminiLiveBackend {
    /// Create a Studio backend with API key authentication.
    pub fn studio(api_key: impl Into<String>) -> Self {
        Self::Studio { api_key: api_key.into() }
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
        Ok(Self::Vertex { credentials, region: region.into(), project_id: project_id.into() })
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
    outbound_tx: mpsc::Sender<Message>,
    writer_task: Arc<Mutex<Option<JoinHandle<()>>>>,
    receiver: Arc<Mutex<WsSource>>,
    audio_buffer: Arc<ParkingMutex<BytesMut>>,
    event_queue: Arc<Mutex<std::collections::VecDeque<ServerEvent>>>,
    schema_cache: Arc<adk_core::SchemaCache>,
    adapter: Arc<dyn adk_core::SchemaAdapter>,
    frame_log: FrameLog,
}

impl GeminiRealtimeSession {
    fn flush_threshold_bytes(format: &AudioFormat) -> usize {
        let bytes_per_second = format.bytes_per_second() as usize;
        // Compute target bytes for a 40ms chunk and round up so we don't under-buffer.
        // `max(1)` keeps the threshold valid even for pathological/invalid formats.
        bytes_per_second.saturating_mul(AUDIO_FLUSH_TARGET_MS).div_ceil(1000).max(1)
    }

    /// Connect to Gemini Live API using the specified backend.
    pub async fn connect(
        backend: GeminiLiveBackend,
        model: &str,
        config: RealtimeConfig,
    ) -> Result<Self> {
        let schema_cache = Arc::new(adk_core::SchemaCache::new());
        let adapter: Arc<dyn adk_core::SchemaAdapter> = match &backend {
            GeminiLiveBackend::Studio { .. } => {
                Arc::new(adk_gemini::schema_adapter::GeminiSchemaAdapter::new())
            }
            #[cfg(feature = "vertex-live")]
            GeminiLiveBackend::Vertex { .. } => {
                Arc::new(adk_gemini::schema_adapter::GeminiSchemaAdapter::vertex_ai())
            }
        };

        // 1. Compile tools BEFORE establishing the WebSocket connection.
        // If any tool fails to compile (semantic loss), we abort early.
        let tools = convert_tools(config.tools.clone(), &schema_cache, adapter.as_ref())?;

        let ws_stream = match &backend {
            GeminiLiveBackend::Studio { api_key } => {
                // `GEMINI_LIVE_URL` is v1beta. On the Studio surface the
                // endpoint version is coupled to the auth method, not the
                // model: API-key auth is served on v1beta, and v1alpha is for
                // ephemeral-token auth. This was hardcoded to v1alpha while
                // the constant went unused — see the Gemini surface table in
                // `zenith/data_plane/AGENTS.md`.
                let url = format!("{}?key={}", super::GEMINI_LIVE_URL, api_key);
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
            GeminiLiveBackend::Vertex { credentials, region, project_id } => {
                let url = build_vertex_live_url(region, project_id)?;

                // Obtain OAuth2 bearer token from ADC credentials
                let header_map =
                    match credentials.headers(Default::default()).await.map_err(|e| {
                        RealtimeError::AuthError(format!(
                            "Failed to obtain OAuth2 token from ADC credentials: {e}"
                        ))
                    })? {
                        google_cloud_auth::credentials::CacheableResource::New { data, .. } => data,
                        google_cloud_auth::credentials::CacheableResource::NotModified => {
                            return Err(RealtimeError::AuthError(
                            "ADC credentials returned NotModified with no cached token available"
                                .to_string(),
                        ));
                        }
                    };

                // Extract the Authorization header value
                let auth_value = header_map
                    .get("authorization")
                    .ok_or_else(|| {
                        RealtimeError::AuthError(
                            "ADC credentials did not produce an Authorization header".to_string(),
                        )
                    })?
                    .to_str()
                    .map_err(|e| {
                        RealtimeError::AuthError(format!(
                            "Authorization header contains non-ASCII characters: {e}"
                        ))
                    })?
                    .to_string();

                // Build a WebSocket request with the Authorization header
                let mut request = url.into_client_request().map_err(|e| {
                    RealtimeError::connection(format!("Failed to create request: {e}"))
                })?;
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
        let (outbound_tx, mut outbound_rx) = mpsc::channel(WRITER_CHANNEL_CAPACITY);
        let writer_connected = Arc::clone(&connected);
        let writer_task = tokio::spawn(async move {
            while let Some(message) = outbound_rx.recv().await {
                // Close frames are terminal for the writer lifecycle: send once then stop.
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
            model.to_string(),
        );

        let session = Self {
            session_id,
            connected,
            outbound_tx,
            writer_task: Arc::new(Mutex::new(Some(writer_task))),
            receiver: Arc::new(Mutex::new(source)),
            audio_buffer: Arc::new(ParkingMutex::new(BytesMut::new())),
            event_queue: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            schema_cache,
            adapter,
            frame_log,
        };

        session.send_setup_with_compiled_tools(model, config, tools).await?;
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

    /// Send initial setup message with pre-compiled tools.
    async fn send_setup_with_compiled_tools(
        &self,
        model: &str,
        config: RealtimeConfig,
        compiled_tools: Option<Vec<Value>>,
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

        // Functionally extract the token if it exists in the prior state map
        let handle = config
            .extra
            .as_ref()
            .and_then(|ext| ext.get("resumeToken"))
            .and_then(|val| val.as_str())
            .map(|s| s.to_string());

        let session_resumption = Some(SessionResumptionConfig { handle });

        // When transcription is requested, enable both input (user speech) and
        // output (model speech) transcription so consumers get clean text for
        // native-audio turns. An empty object turns the feature on.
        let transcription = config.input_audio_transcription.as_ref().map(|_| json!({}));

        let setup = GeminiClientMessage {
            setup: Some(GeminiSetup {
                model: Some(model.to_string()),
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

        tracing::info!(model_id = %model, "Sending Gemini Live setup");
        self.send_raw(&setup).await
    }

    /// Send a raw message.
    async fn send_raw<T: Serialize>(&self, value: &T) -> Result<()> {
        let msg = serde_json::to_string(value)
            .map_err(|e| RealtimeError::protocol(format!("JSON serialize error: {}", e)))?;

        self.outbound_tx
            .send(Message::Text(msg.into()))
            .await
            .map_err(|e| RealtimeError::connection(format!("Send queue error: {e}")))?;

        Ok(())
    }

    /// Receive and parse the next message.
    async fn receive_raw(&self) -> Option<Result<ServerEvent>> {
        // First check if there's anything in the queue
        {
            let mut queue = self.event_queue.lock().await;
            if let Some(event) = queue.pop_front() {
                return Some(Ok(event));
            }
        }

        let mut receiver = self.receiver.lock().await;

        match receiver.next().await {
            Some(Ok(Message::Text(text))) => match self.translate_gemini_event(&text) {
                Ok(events) => {
                    let mut queue = self.event_queue.lock().await;
                    let mut iter = events.into_iter();
                    let first = iter.next();
                    for event in iter {
                        queue.push_back(event);
                    }
                    first.map(Ok)
                }
                Err(e) => Some(Err(e)),
            },
            Some(Ok(Message::Binary(bytes))) => match String::from_utf8(bytes.to_vec()) {
                Ok(text) => match self.translate_gemini_event(&text) {
                    Ok(events) => {
                        let mut queue = self.event_queue.lock().await;
                        let mut iter = events.into_iter();
                        let first = iter.next();
                        for event in iter {
                            queue.push_back(event);
                        }
                        first.map(Ok)
                    }
                    Err(e) => Some(Err(e)),
                },
                Err(e) => Some(Err(RealtimeError::protocol(format!(
                    "Invalid UTF-8 in binary message: {}",
                    e
                )))),
            },
            Some(Ok(Message::Close(close_frame))) => {
                tracing::error!("WebSocket closed by server: {:?}", close_frame);
                self.connected.store(false, Ordering::SeqCst);
                None
            }
            Some(Ok(msg)) => {
                tracing::warn!("Received unhandled tungstenite message: {:?}", msg);
                Some(Ok(ServerEvent::Unknown))
            }
            Some(Err(e)) => {
                self.connected.store(false, Ordering::SeqCst);
                Some(Err(RealtimeError::connection(format!("Receive error: {}", e))))
            }
            None => {
                self.connected.store(false, Ordering::SeqCst);
                None
            }
        }
    }

    /// Translate Gemini-specific events to unified format.
    fn translate_gemini_event(&self, raw: &str) -> Result<Vec<ServerEvent>> {
        Self::translate_event_logged(raw, Some(&self.frame_log))
    }

    /// Test entry point: no session, so no per-connection telemetry identity.
    #[cfg(test)]
    pub(crate) fn translate_event_static(raw: &str) -> Result<Vec<ServerEvent>> {
        Self::translate_event_logged(raw, None)
    }

    pub(crate) fn translate_event_logged(
        raw: &str,
        frame_log: Option<&FrameLog>,
    ) -> Result<Vec<ServerEvent>> {
        tracing::debug!(%raw, "Translating Gemini event");
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

        // Check for server content (audio/text)
        if let Some(content) = value.get("serverContent") {
            let mut events = Vec::new();

            // Barge-in: the caller spoke over the model, so this generation is
            // abandoned and the client must empty its playback queue.
            let interrupted = content.get("interrupted").and_then(|i| i.as_bool()).unwrap_or(false);
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
                (true, Some(handle)) => {
                    tracing::debug!("Received a resumable Gemini session handle");
                    return Ok(vec![ServerEvent::SessionUpdated {
                        event_id: uuid::Uuid::new_v4().to_string(),
                        session: json!({ "resumeHandle": handle }),
                    }]);
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
impl RealtimeSession for GeminiRealtimeSession {
    fn session_id(&self) -> &str {
        &self.session_id
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
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
        // Use client_content with turns (correct Gemini Live API format)
        let msg = GeminiClientMessage {
            setup: None,
            realtime_input: None,
            tool_response: None,
            client_content: Some(GeminiClientContent {
                turns: vec![GeminiTurn {
                    role: "user".to_string(),
                    parts: vec![GeminiPart { text: Some(text.to_string()), inline_data: None }],
                }],
                turn_complete: true,
            }),
        };
        self.send_raw(&msg).await
    }

    async fn send_tool_response(&self, response: ToolResponse) -> Result<()> {
        let output = match &response.output {
            Value::String(s) => json!({ "result": s }),
            other => other.clone(),
        };

        let msg = GeminiClientMessage {
            setup: None,
            realtime_input: None,
            tool_response: Some(GeminiToolResponse {
                function_responses: vec![GeminiFunctionResponse {
                    id: response.call_id,
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

                let handle = config
                    .extra
                    .as_ref()
                    .and_then(|ext| ext.get("resumeToken"))
                    .and_then(|val| val.as_str())
                    .map(|s| s.to_string());

                let session_resumption =
                    if handle.is_some() { Some(SessionResumptionConfig { handle }) } else { None };

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
        // Route close through the same channel as normal writes so ordering is preserved.
        let _ = self.outbound_tx.send(Message::Close(None)).await;

        let mut writer_task = self.writer_task.lock().await;
        if let Some(handle) = writer_task.take() {
            // Ensure deterministic teardown: don't return until the writer released the sink.
            let _ = handle.await;
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
            adk_core::types::Part::InlineData { mime_type, data } => {
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
        if let Some(params) = &t.parameters {
            let compiled = cache.get_or_compile(params, adapter).map_err(|e| {
                RealtimeError::protocol(format!(
                    "Failed to compile schema for tool {}: {}",
                    t.name, e
                ))
            })?;
            decl["parameters"] = compiled;
        } else {
            decl["parameters"] = adapter.empty_schema();
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
            Part::InlineData { mime_type: "image/png".to_string(), data: vec![0x1, 0x2] },
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

    /// The field the server actually sends. `resumptionToken` was never sent, so
    /// this branch never fired and no handle was ever captured.
    #[test]
    fn resumable_session_update_yields_the_new_handle() {
        let raw = r#"{"sessionResumptionUpdate":{"newHandle":"handle-abc","resumable":true}}"#;
        let events = GeminiRealtimeSession::translate_event_static(raw).unwrap();

        match events.as_slice() {
            [ServerEvent::SessionUpdated { session, .. }] => {
                assert_eq!(
                    session.get("resumeHandle").and_then(|h| h.as_str()),
                    Some("handle-abc")
                );
            }
            other => panic!("expected one SessionUpdated, got {other:?}"),
        }
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
            Part::InlineData { mime_type: "image/png".to_string(), data: vec![0x1] },
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
        let parts = vec![Part::InlineData { mime_type: "image/png".to_string(), data: vec![0x1] }];
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
}
