# Realtime Session Architecture & Cognitive Handoffs

This document outlines the session management architecture for `adk-realtime`—specifically detailing how "Cognitive Handoffs" (mid-flight context and tool mutations) are handled across different LLM providers (e.g., OpenAI and Google Gemini) without disrupting the upstream audio transport (like LiveKit WebRTC).

---

## 1. The Core Challenge: Mid-Flight Context Mutation

When building conversational AI systems (like IVRs or voice assistants), it is frequently necessary to shift the agent's behavior dynamically based on user intent. For example, moving a user from a "Receptionist" persona (which only has routing tools) to a "Billing" persona (which has refund processing tools).

To do this efficiently, the system must update the LLM's **System Instructions** and **Available Tools** over an active, full-duplex voice connection *without* causing unacceptable latency or dropping the user's call.

### The Protocol Asymmetry
Different AI providers handle streaming WebSocket/WebRTC architectures completely differently:

*   **OpenAI Realtime API:** Native Mutability. OpenAI allows dynamic injection or removal of tools mid-stream using a `session.update` frame. The model recalculates its available action space on the fly.
*   **Google Gemini Live API:** Rigid Initialization. Gemini strictly bounds the `tools` array and `systemInstruction` to the initial `setup` frame. Attempting to mutate the tools mid-flight or send a second `setup` frame results in a fatal socket disconnection. However, Gemini *does* support **Session Resumption** via a `resumeToken`, allowing a new connection to instantly inherit the conversational history of an older, dropped connection.

---

## 2. The `adk-realtime` Solution: Polymorphic Orchestration

To harmonize these fundamentally incompatible paradigms, `adk-realtime` implements a **Provider-Agnostic Cognitive Handoff** using a capability-driven trait pattern. The central `RealtimeRunner` manages the generic session, while the provider-specific session implementations explicitly dictate their mutation capabilities.

### A. The Contract: `ContextMutationOutcome`
The `RealtimeSession::mutate_context` method does not return a simple success/error flag. Instead, it returns a semantic outcome defining how the framework must react:

```rust
pub enum ContextMutationOutcome {
    /// The provider natively hot-swapped the context over the active connection.
    Applied,
    /// The provider requires the transport to be torn down and rebuilt.
    RequiresResumption(RealtimeConfig),
}
```

*   **OpenAI** returns `ContextMutationOutcome::Applied`. The runner does nothing further.
*   **Gemini** returns `ContextMutationOutcome::RequiresResumption`. This triggers the runner's internal "Phantom Reconnect" sequence.

### B. The "Phantom Reconnect" (Safe Session Resumption)
When a provider returns `RequiresResumption`, the `RealtimeRunner` executes a seamless teardown and rebuild of the LLM WebSocket while keeping the upstream audio pipe (e.g., LiveKit) active.

1.  **State Machine Safety:** The runner maintains a `RunnerState` (`Idle`, `Generating`, `ExecutingTool`). If a resumption is requested while the LLM is busy generating audio or waiting for a tool execution, the runner **queues** the mutation as a `PendingResumption`. Tearing down a socket mid-generation corrupts the conversational context and causes data loss.
2.  **The Queue Policy:** The queue implements a **"Last Write Wins"** policy. If multiple context shifts arrive during a busy state, only the newest configuration is kept. This ensures the runner converges on the final desired state rather than processing a stale history of commands.
3.  **Bounded Retries:** If the teardown/rebuild fails (due to transient network issues), the runner catches the failure and restores the pending queue. It limits retries to prevent catastrophic infinite-looping.
4.  **Resumption Tokens:** The runner explicitly caches provider-specific metadata (like Gemini's `sessionResumptionUpdate` token) in its internal `config.extra` map. When the "Phantom Reconnect" establishes the new connection, the token is automatically injected into the new `setup` frame, preserving the caller's short-term memory natively.

### C. Bridge Messages
To prevent "attention shock" when a model's persona shifts drastically, orchestrators can attach an optional `bridge_message` to a queued resumption. Once the new transport is established, the runner immediately injects this message into the new context window (e.g., `[SYSTEM EVENT: Context shifted to Billing]`).

---

## 3. Implementing a New Provider Model

If you are implementing a new AI provider inside `adk-realtime`, follow these steps to leverage the existing architecture:

1.  **Implement `RealtimeSession::mutate_context`:**
    *   Does the provider support sending a native JSON frame to update tools mid-stream? If yes, serialize your payload, call `self.send_raw(...)`, and return `Ok(ContextMutationOutcome::Applied)`.
    *   Does the provider lock the configuration at connection time? Return `Ok(ContextMutationOutcome::RequiresResumption(config))`. The `RealtimeRunner` will automatically handle the complex teardown/rebuild lifecycle for you.

2.  **Translate `ClientEvent::Message`:**
    *   The `adk-core` ecosystem uses a universal `ClientEvent::Message` intent utilizing strongly-typed `Role` strings and `adk_core::types::Part` structs.
    *   Your provider's `send_event` implementation must intercept this event and structurally map it into the native wire format (e.g., OpenAI's `conversation.item.create` or Gemini's `clientContent.turns`).
    *   **Crucial:** Do not forward raw internal intents (like `ClientEvent::UpdateSession`) to the provider socket. The `RealtimeRunner` intercepts these orchestration intents directly.

3.  **Surface Resumption Tokens (Optional):**
    *   If your provider supports session resumption (like Gemini), parse the incoming server events in `translate_event` or `receive_raw`.
    *   When a token arrives, emit a `ServerEvent::SessionUpdated { session: json!({ "resumeToken": "your_token" }) }`.
    *   The `RealtimeRunner` will automatically catch this event, persist the token in `config.extra["resumeToken"]`, and pass it back to your implementation's `connect()` method during the next Phantom Reconnect.
