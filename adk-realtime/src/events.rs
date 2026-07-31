//! Event types for realtime communication.
//!
//! These events follow a unified model inspired by the OpenAI Agents SDK,
//! abstracting over provider-specific event formats.
//!
//! Audio data is transported as raw bytes (`Vec<u8>`) internally but serialized
//! as base64 on the wire for JSON compatibility.

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Custom serde for base64-encoded audio ───────────────────────────────

fn deserialize_audio_bytes<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    base64::engine::general_purpose::STANDARD.decode(&s).map_err(serde::de::Error::custom)
}

fn serialize_audio_bytes<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let s = base64::engine::general_purpose::STANDARD.encode(bytes);
    serializer.serialize_str(&s)
}

// ── Client Events ───────────────────────────────────────────────────────

/// Events sent from the client to the realtime server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum ClientEvent {
    /// Update session configuration.
    #[serde(rename = "session.update")]
    SessionUpdate {
        /// Updated session configuration.
        session: Value,
    },

    /// Append audio to the input buffer.
    #[serde(rename = "input_audio_buffer.append")]
    AudioDelta {
        /// Optional event ID.
        #[serde(skip_serializing_if = "Option::is_none")]
        event_id: Option<String>,
        /// Audio data (raw bytes, serialized as base64 on the wire).
        #[serde(
            serialize_with = "serialize_audio_bytes",
            deserialize_with = "deserialize_audio_bytes"
        )]
        audio: Vec<u8>,
        /// Audio format metadata for multi-format pipelines and debugging.
        /// Skipped during serialization — the server infers format from the session config.
        #[serde(skip)]
        format: Option<crate::audio::AudioFormat>,
    },

    /// Commit the current audio buffer (manual mode).
    #[serde(rename = "input_audio_buffer.commit")]
    InputAudioBufferCommit,

    /// Clear the audio input buffer.
    #[serde(rename = "input_audio_buffer.clear")]
    InputAudioBufferClear,

    /// Send a text message or tool response.
    #[serde(rename = "conversation.item.create")]
    ConversationItemCreate {
        /// The conversation item (flexible JSON for provider compatibility).
        item: Value,
    },

    /// Trigger a response from the model.
    #[serde(rename = "response.create")]
    ResponseCreate {
        /// Optional response configuration.
        #[serde(skip_serializing_if = "Option::is_none")]
        config: Option<Value>,
    },

    /// Cancel/interrupt the current response.
    #[serde(rename = "response.cancel")]
    ResponseCancel,

    /// A standard message using `adk_core`'s native Role and Part types.
    #[serde(rename = "message")]
    Message {
        /// Role of the message.
        role: String,
        /// Content parts of the message.
        parts: Vec<adk_core::types::Part>,
    },

    /// Universal intent to update session configuration mid-flight.
    ///
    /// This is treated as a runner/control-plane internal intent and should not
    /// be sent directly to providers without interception. By construction, it
    /// is explicitly untagged from serialization to guarantee it cannot
    /// leak onto the WebSocket wire.
    #[serde(skip_serializing)]
    UpdateSession {
        /// New system instructions.
        #[serde(skip_serializing_if = "Option::is_none")]
        instructions: Option<String>,
        /// New tools definition.
        #[serde(skip_serializing_if = "Option::is_none")]
        tools: Option<Vec<crate::config::ToolDefinition>>,
    },
}

/// A conversation item for text or tool responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationItem {
    /// Unique ID for this item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Item type: "message" or "function_call_output".
    #[serde(rename = "type")]
    pub item_type: String,
    /// Role: "user", "assistant", or "system".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Content parts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<ContentPart>>,
    /// For tool responses: the call ID being responded to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    /// For tool responses: the output value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

/// A content part within a conversation item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentPart {
    /// Content type: "input_text", "input_audio", "text", "audio".
    #[serde(rename = "type")]
    pub content_type: String,
    /// Text content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Base64-encoded audio content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<String>,
    /// Transcript of audio content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
}

impl ConversationItem {
    /// Create a user text message item.
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            id: None,
            item_type: "message".to_string(),
            role: Some("user".to_string()),
            content: Some(vec![ContentPart {
                content_type: "input_text".to_string(),
                text: Some(text.into()),
                audio: None,
                transcript: None,
            }]),
            call_id: None,
            output: None,
        }
    }

    /// Create a tool response item.
    pub fn tool_response(call_id: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            id: None,
            item_type: "function_call_output".to_string(),
            role: None,
            content: None,
            call_id: Some(call_id.into()),
            output: Some(output.into()),
        }
    }
}

// ── Server Events ───────────────────────────────────────────────────────

/// Identifier of a provider-issued function call.
///
/// A newtype rather than a bare `String` because these are matched against
/// each other to decide whether a side effect may proceed, and a
/// provider-supplied identity that is empty is not an identity. An adapter
/// that filled such a field with `String::new()` once let an empty value
/// compare equal to another empty value across three layers before anything
/// failed; [`ToolCallId::parse`] makes that unrepresentable at the boundary
/// instead of surprising at the comparison.
///
/// Deserialization goes through the same check, which is safe here because
/// only providers that emit a cancellation frame construct one — the OpenAI
/// wire format, which is deserialized directly into [`ServerEvent`], has no
/// `tool_call.cancelled` message.
///
/// # Example
///
/// ```
/// use adk_realtime::events::ToolCallId;
///
/// let id = ToolCallId::parse("call_1").expect("non-empty");
/// assert_eq!(id.as_str(), "call_1");
/// assert!(ToolCallId::parse("   ").is_err());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ToolCallId(String);

impl ToolCallId {
    /// Accept a provider-supplied call id, rejecting one that carries no
    /// identity.
    pub fn parse(value: impl Into<String>) -> std::result::Result<Self, EmptyToolCallId> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(EmptyToolCallId);
        }
        Ok(Self(value))
    }

    /// Borrow the underlying identifier, for comparison against a `call_id`
    /// carried by the not-yet-typed variants of this enum.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ToolCallId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A tool call id was empty or whitespace-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("tool call id is empty")]
pub struct EmptyToolCallId;

impl<'de> Deserialize<'de> for ToolCallId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

/// Events received from the realtime server.
///
/// This is a unified event type that abstracts over provider-specific formats.
/// Audio data is stored as raw bytes (`Vec<u8>`) — decoded from base64 at the
/// transport boundary so consumers never need to deal with encoding.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum ServerEvent {
    /// Session was created/connected.
    #[serde(rename = "session.created")]
    SessionCreated {
        /// Unique event ID.
        event_id: String,
        /// Session details.
        session: Value,
    },

    /// Session configuration was updated.
    #[serde(rename = "session.updated")]
    SessionUpdated {
        /// Unique event ID.
        event_id: String,
        /// Updated session details.
        session: Value,
    },

    /// Error occurred.
    #[serde(rename = "error")]
    Error {
        /// Unique event ID.
        event_id: String,
        /// Error details.
        error: ErrorInfo,
    },

    /// User speech started (VAD detected).
    #[serde(rename = "input_audio_buffer.speech_started")]
    SpeechStarted {
        /// Unique event ID.
        event_id: String,
        /// Audio start time in milliseconds.
        audio_start_ms: u64,
    },

    /// User speech ended (VAD detected).
    #[serde(rename = "input_audio_buffer.speech_stopped")]
    SpeechStopped {
        /// Unique event ID.
        event_id: String,
        /// Audio end time in milliseconds.
        audio_end_ms: u64,
    },

    /// Audio input buffer was committed.
    #[serde(rename = "input_audio_buffer.committed")]
    AudioCommitted {
        /// Unique event ID.
        event_id: String,
        /// ID of the created item.
        item_id: String,
    },

    /// Audio input buffer was cleared.
    #[serde(rename = "input_audio_buffer.cleared")]
    AudioCleared {
        /// Unique event ID.
        event_id: String,
    },

    /// Conversation item was created.
    #[serde(rename = "conversation.item.created")]
    ItemCreated {
        /// Unique event ID.
        event_id: String,
        /// The created item.
        item: Value,
    },

    /// Response generation started.
    #[serde(rename = "response.created")]
    ResponseCreated {
        /// Unique event ID.
        event_id: String,
        /// Response details.
        response: Value,
    },

    /// Response generation completed.
    #[serde(rename = "response.done")]
    ResponseDone {
        /// Unique event ID.
        event_id: String,
        /// Final response details.
        response: Value,
    },

    /// The in-flight response was cancelled before completion, normally
    /// because the caller began speaking over it (barge-in).
    ///
    /// This is a normal lifecycle boundary, not an error. Consumers owning an
    /// audio sink must discard anything already queued for playback: the
    /// provider has abandoned that turn, and continuing to play it means
    /// talking over the caller.
    ///
    /// Gemini signals this as `serverContent.interrupted`; the field was
    /// previously parsed and dropped, so `EventHandler::on_response_cancelled`
    /// could never fire on that backend.
    #[serde(rename = "response.cancelled")]
    ResponseCancelled {
        /// Unique event ID.
        event_id: String,
    },

    /// The provider withdrew function calls it had already issued, normally
    /// because the caller interrupted the turn that produced them.
    ///
    /// The ids correspond to `call_id` values previously delivered by
    /// [`ServerEvent::FunctionCallDone`]. Per Google's Live API, a cancelled
    /// call "should not have been executed"; a client that already performed a
    /// side effect may have to undo it. Ignoring this event means a request the
    /// caller withdrew can still take effect.
    ///
    /// The runner surfaces this but does not itself abort work already
    /// dispatched — whether an in-flight effect can be cancelled or must be
    /// compensated is the application's decision, not the transport's.
    ///
    #[serde(rename = "tool_call.cancelled")]
    ToolCallCancelled {
        /// Ids of the function calls being withdrawn.
        call_ids: Vec<ToolCallId>,
    },

    /// Response output item added.
    #[serde(rename = "response.output_item.added")]
    OutputItemAdded {
        /// Unique event ID.
        event_id: String,
        /// Response ID.
        response_id: String,
        /// Output index.
        output_index: u32,
        /// The output item.
        item: Value,
    },

    /// Response output item completed.
    #[serde(rename = "response.output_item.done")]
    OutputItemDone {
        /// Unique event ID.
        event_id: String,
        /// Response ID.
        response_id: String,
        /// Output index.
        output_index: u32,
        /// The completed item.
        item: Value,
    },

    /// Audio delta (chunk of output audio as raw bytes).
    #[serde(alias = "response.audio.delta", rename = "response.output_audio.delta")]
    AudioDelta {
        /// Unique event ID.
        event_id: String,
        /// Response ID.
        response_id: String,
        /// Item ID.
        item_id: String,
        /// Output index.
        output_index: u32,
        /// Content index.
        content_index: u32,
        /// Audio data (raw bytes, serialized as base64 on the wire).
        #[serde(
            serialize_with = "serialize_audio_bytes",
            deserialize_with = "deserialize_audio_bytes"
        )]
        delta: Vec<u8>,
    },

    /// Audio output completed.
    #[serde(alias = "response.audio.done", rename = "response.output_audio.done")]
    AudioDone {
        /// Unique event ID.
        event_id: String,
        /// Response ID.
        response_id: String,
        /// Item ID.
        item_id: String,
        /// Output index.
        output_index: u32,
        /// Content index.
        content_index: u32,
    },

    /// Text delta (chunk of output text).
    #[serde(alias = "response.text.delta", rename = "response.output_text.delta")]
    TextDelta {
        /// Unique event ID.
        event_id: String,
        /// Response ID.
        response_id: String,
        /// Item ID.
        item_id: String,
        /// Output index.
        output_index: u32,
        /// Content index.
        content_index: u32,
        /// Text content.
        delta: String,
    },

    /// Text output completed.
    #[serde(alias = "response.text.done", rename = "response.output_text.done")]
    TextDone {
        /// Unique event ID.
        event_id: String,
        /// Response ID.
        response_id: String,
        /// Item ID.
        item_id: String,
        /// Output index.
        output_index: u32,
        /// Content index.
        content_index: u32,
        /// Complete text.
        text: String,
    },

    /// Audio transcript delta.
    #[serde(
        alias = "response.audio_transcript.delta",
        rename = "response.output_audio_transcript.delta"
    )]
    TranscriptDelta {
        /// Unique event ID.
        event_id: String,
        /// Response ID.
        response_id: String,
        /// Item ID.
        item_id: String,
        /// Output index.
        output_index: u32,
        /// Content index.
        content_index: u32,
        /// Transcript delta.
        delta: String,
    },

    /// Audio transcript completed.
    #[serde(
        alias = "response.audio_transcript.done",
        rename = "response.output_audio_transcript.done"
    )]
    TranscriptDone {
        /// Unique event ID.
        event_id: String,
        /// Response ID.
        response_id: String,
        /// Item ID.
        item_id: String,
        /// Output index.
        output_index: u32,
        /// Content index.
        content_index: u32,
        /// Complete transcript.
        transcript: String,
    },

    /// Function call arguments delta.
    #[serde(rename = "response.function_call_arguments.delta")]
    FunctionCallDelta {
        /// Unique event ID.
        event_id: String,
        /// Response ID.
        response_id: String,
        /// Item ID.
        item_id: String,
        /// Output index.
        output_index: u32,
        /// Call ID.
        call_id: String,
        /// Arguments delta.
        delta: String,
    },

    /// Function call completed.
    #[serde(rename = "response.function_call_arguments.done")]
    FunctionCallDone {
        /// Unique event ID.
        event_id: String,
        /// Response ID.
        response_id: String,
        /// Item ID.
        item_id: String,
        /// Output index.
        output_index: u32,
        /// Call ID.
        call_id: String,
        /// Function name.
        name: String,
        /// Complete arguments.
        arguments: Value,
    },

    /// Rate limit information.
    #[serde(rename = "rate_limits.updated")]
    RateLimitsUpdated {
        /// Unique event ID.
        event_id: String,
        /// Rate limit details.
        rate_limits: Vec<RateLimit>,
    },

    /// GA API: User input audio transcription delta (streaming partial transcript).
    #[serde(rename = "conversation.item.input_audio_transcription.delta")]
    InputTranscriptDelta {
        /// Item ID for the input audio item being transcribed.
        item_id: String,
        /// Content index.
        content_index: u32,
        /// Partial transcript text.
        delta: String,
    },

    /// GA API: User input audio transcription completed (final transcript).
    #[serde(rename = "conversation.item.input_audio_transcription.completed")]
    InputTranscriptCompleted {
        /// Item ID for the input audio item.
        item_id: String,
        /// Content index.
        content_index: u32,
        /// Final complete transcript.
        transcript: String,
    },

    /// Unknown event type (for forward compatibility).
    #[serde(other)]
    Unknown,
}

/// Error information from the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorInfo {
    /// Error type/code.
    #[serde(rename = "type")]
    pub error_type: String,
    /// Error code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Human-readable error message.
    pub message: String,
    /// Additional error parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
}

/// Rate limit information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimit {
    /// Limit name.
    pub name: String,
    /// Maximum allowed.
    pub limit: u64,
    /// Currently remaining.
    pub remaining: u64,
    /// Time until reset.
    pub reset_seconds: f64,
}

/// A simplified tool call representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique call ID (used for responses).
    pub call_id: String,
    /// Tool/function name.
    pub name: String,
    /// Arguments as JSON.
    pub arguments: Value,
}

/// A tool response to send back to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResponse {
    /// The call ID being responded to.
    pub call_id: String,
    /// The result/output of the tool execution.
    pub output: Value,
}

impl ToolResponse {
    /// Create a new tool response.
    pub fn new(call_id: impl Into<String>, output: impl Serialize) -> Self {
        Self {
            call_id: call_id.into(),
            output: serde_json::to_value(output).unwrap_or(Value::Null),
        }
    }

    /// Create a tool response from a string output.
    pub fn from_string(call_id: impl Into<String>, output: impl Into<String>) -> Self {
        Self { call_id: call_id.into(), output: Value::String(output.into()) }
    }
}
