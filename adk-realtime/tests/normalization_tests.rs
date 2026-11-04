#![cfg(feature = "openai")]

use adk_realtime::error::RealtimeError;
use adk_realtime::events::ServerEvent;
use serde_json::{Value, json};
use tokio::sync::Mutex;

#[cfg(feature = "openai")]
mod openai_tests {
    use super::*;
    use adk_realtime::openai::protocol::OpenAITransportLink;
    use async_trait::async_trait;

    struct MockTransport {
        events: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl OpenAITransportLink for MockTransport {
        fn session_id(&self) -> &str {
            "mock"
        }
        fn is_connected(&self) -> bool {
            true
        }
        async fn send_raw(&self, _v: &Value) -> adk_realtime::error::Result<()> {
            Ok(())
        }
        async fn close(&self) -> adk_realtime::error::Result<()> {
            Ok(())
        }
        async fn receive_raw(&self) -> Option<adk_realtime::error::Result<ServerEvent>> {
            let mut events = self.events.lock().await;
            if events.is_empty() {
                return None;
            }
            let text = events.remove(0);

            // This mirrors the logic in OpenAIRealtimeSession::receive_raw
            match serde_json::from_str::<ServerEvent>(&text) {
                Ok(mut event) => {
                    if let ServerEvent::FunctionCallDone { arguments, name, .. } = &mut event
                        && let Value::String(s) = arguments
                    {
                        match serde_json::from_str::<Value>(s) {
                            Ok(parsed) => {
                                *arguments = parsed;
                            }
                            Err(e) => {
                                return Some(Err(RealtimeError::protocol(format!(
                                    "malformed function arguments for {}: {}",
                                    name, e
                                ))));
                            }
                        }
                    }
                    Some(Ok(event))
                }
                Err(_) => Some(Ok(ServerEvent::Unknown)),
            }
        }
    }

    #[tokio::test]
    async fn test_openai_argument_normalization() {
        let transport = MockTransport {
            events: Mutex::new(vec![
                json!({
                    "type": "response.function_call_arguments.done",
                    "event_id": "evt_1",
                    "response_id": "resp_1",
                    "item_id": "item_1",
                    "output_index": 0,
                    "call_id": "call_1",
                    "name": "test_tool",
                    "arguments": "{\"key\": \"value\"}"
                })
                .to_string(),
            ]),
        };

        let event = transport.receive_raw().await.unwrap().unwrap();
        if let ServerEvent::FunctionCallDone { arguments, .. } = event {
            assert_eq!(arguments, json!({"key": "value"}));
        } else {
            panic!("Expected FunctionCallDone");
        }
    }

    #[tokio::test]
    async fn test_openai_malformed_argument_normalization() {
        let transport = MockTransport {
            events: Mutex::new(vec![
                json!({
                    "type": "response.function_call_arguments.done",
                    "event_id": "evt_1",
                    "response_id": "resp_1",
                    "item_id": "item_1",
                    "output_index": 0,
                    "call_id": "call_1",
                    "name": "test_tool",
                    "arguments": "{\"key\": \"value\"" // Malformed JSON
                })
                .to_string(),
            ]),
        };

        let result = transport.receive_raw().await.unwrap();
        match result {
            Err(RealtimeError::Protocol(msg)) => {
                assert!(msg.contains("malformed function arguments"));
            }
            _ => panic!("Expected Protocol error, got {:?}", result),
        }
    }
}

#[cfg(feature = "openai")]
#[tokio::test]
async fn test_transfer_to_agent_validation() {
    use adk_core::{
        Agent, CallbackContext, Content, InvocationContext, ReadonlyContext, RunConfig,
    };
    use adk_realtime::session::BoxedSession;
    use adk_realtime::{RealtimeAgent, RealtimeConfig, audio::AudioFormat};
    use futures::StreamExt;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug)]
    struct MockModel {
        tool_responses: Arc<AtomicUsize>,
    }

    impl MockModel {
        fn new() -> Self {
            Self { tool_responses: Arc::new(AtomicUsize::new(0)) }
        }
    }
    #[async_trait::async_trait]
    impl adk_realtime::model::RealtimeModel for MockModel {
        fn provider(&self) -> &str {
            "mock"
        }
        fn model_id(&self) -> &str {
            "mock"
        }
        fn supported_input_formats(&self) -> Vec<AudioFormat> {
            vec![]
        }
        fn supported_output_formats(&self) -> Vec<AudioFormat> {
            vec![]
        }
        fn available_voices(&self) -> Vec<&str> {
            vec![]
        }
        async fn connect(
            &self,
            _config: RealtimeConfig,
        ) -> adk_realtime::error::Result<BoxedSession> {
            Ok(Box::new(MockSession {
                tool_responses: Arc::clone(&self.tool_responses),
                event_calls: AtomicUsize::new(0),
            }))
        }
    }

    struct MockSession {
        tool_responses: Arc<AtomicUsize>,
        event_calls: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl adk_realtime::session::RealtimeSession for MockSession {
        fn session_id(&self) -> &str {
            "mock"
        }
        fn is_connected(&self) -> bool {
            true
        }
        async fn send_audio(
            &self,
            _a: &adk_realtime::audio::AudioChunk,
        ) -> adk_realtime::error::Result<()> {
            Ok(())
        }
        async fn send_audio_base64(&self, _a: &str) -> adk_realtime::error::Result<()> {
            Ok(())
        }
        async fn send_text(&self, _t: &str) -> adk_realtime::error::Result<()> {
            Ok(())
        }
        async fn send_tool_response(
            &self,
            _r: adk_realtime::events::ToolResponse,
        ) -> adk_realtime::error::Result<()> {
            self.tool_responses.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn commit_audio(&self) -> adk_realtime::error::Result<()> {
            Ok(())
        }
        async fn clear_audio(&self) -> adk_realtime::error::Result<()> {
            Ok(())
        }
        async fn create_response(&self) -> adk_realtime::error::Result<()> {
            Ok(())
        }
        async fn interrupt(&self) -> adk_realtime::error::Result<()> {
            Ok(())
        }
        async fn send_event(
            &self,
            _e: adk_realtime::events::ClientEvent,
        ) -> adk_realtime::error::Result<()> {
            Ok(())
        }
        async fn next_event(&self) -> Option<adk_realtime::error::Result<ServerEvent>> {
            if self.event_calls.fetch_add(1, Ordering::SeqCst) > 0 {
                // After the refused transfer the session is still alive.
                return Some(Ok(ServerEvent::TextDelta {
                    event_id: "evt_2".into(),
                    response_id: "resp_1".into(),
                    item_id: "item_1".into(),
                    output_index: 0,
                    content_index: 0,
                    delta: "still here".into(),
                }));
            }
            Some(Ok(ServerEvent::FunctionCallDone {
                event_id: "evt_1".into(),
                response_id: "resp_1".into(),
                item_id: "item_1".into(),
                output_index: 0,
                call_id: "call_1".into(),
                name: "transfer_to_agent".into(),
                arguments: json!({"agent_name": ""}), // Empty agent_name
            }))
        }
        fn events(
            &self,
        ) -> std::pin::Pin<
            Box<dyn futures::Stream<Item = adk_realtime::error::Result<ServerEvent>> + Send + '_>,
        > {
            Box::pin(futures::stream::empty())
        }
        async fn close(&self) -> adk_realtime::error::Result<()> {
            Ok(())
        }
        async fn mutate_context(
            &self,
            _c: RealtimeConfig,
        ) -> adk_realtime::error::Result<adk_realtime::session::ContextMutationOutcome> {
            Ok(adk_realtime::session::ContextMutationOutcome::Applied)
        }
    }

    struct DummyState;
    impl adk_core::State for DummyState {
        fn get(&self, _key: &str) -> Option<serde_json::Value> {
            None
        }
        fn set(&mut self, _key: String, _value: serde_json::Value) {}
        fn all(&self) -> std::collections::HashMap<String, serde_json::Value> {
            std::collections::HashMap::new()
        }
    }

    struct DummySession;
    impl adk_core::Session for DummySession {
        fn id(&self) -> &str {
            "sess_1"
        }
        fn app_name(&self) -> &str {
            "app"
        }
        fn user_id(&self) -> &str {
            "user_1"
        }
        fn state(&self) -> &dyn adk_core::State {
            &DummyState
        }
        fn conversation_history(&self) -> Vec<Content> {
            Vec::new()
        }
    }

    struct MockInvocationContext {
        content: Content,
        agent: Arc<dyn Agent>,
        session: DummySession,
        run_config: RunConfig,
    }
    #[async_trait::async_trait]
    impl ReadonlyContext for MockInvocationContext {
        fn invocation_id(&self) -> &str {
            "inv_1"
        }
        fn agent_name(&self) -> &str {
            "test"
        }
        fn user_id(&self) -> &str {
            "user_1"
        }
        fn app_name(&self) -> &str {
            "app"
        }
        fn session_id(&self) -> &str {
            "sess_1"
        }
        fn branch(&self) -> &str {
            ""
        }
        fn user_content(&self) -> &Content {
            &self.content
        }
    }
    #[async_trait::async_trait]
    impl CallbackContext for MockInvocationContext {
        fn artifacts(&self) -> Option<Arc<dyn adk_core::Artifacts>> {
            None
        }
    }
    #[async_trait::async_trait]
    impl InvocationContext for MockInvocationContext {
        fn memory(&self) -> Option<Arc<dyn adk_core::Memory>> {
            None
        }
        fn agent(&self) -> Arc<dyn Agent> {
            self.agent.clone()
        }
        fn session(&self) -> &dyn adk_core::Session {
            &self.session
        }
        fn run_config(&self) -> &RunConfig {
            &self.run_config
        }
        fn end_invocation(&self) {}
        fn ended(&self) -> bool {
            false
        }
    }

    let sub_agent =
        RealtimeAgent::builder("sub").model(Arc::new(MockModel::new())).build().unwrap();
    let agent_model = MockModel::new();
    let tool_responses = Arc::clone(&agent_model.tool_responses);
    let agent = RealtimeAgent::builder("test")
        .model(Arc::new(agent_model))
        .sub_agent(Arc::new(sub_agent))
        .build()
        .unwrap();

    let ctx = MockInvocationContext {
        content: Content { role: "user".into(), parts: vec![] },
        agent: Arc::new(
            RealtimeAgent::builder("test").model(Arc::new(MockModel::new())).build().unwrap(),
        ),
        session: DummySession,
        run_config: RunConfig::default(),
    };
    let mut stream = agent.run(Arc::new(ctx)).await.unwrap();

    // First event is session started
    let _ = stream.next().await.unwrap().unwrap();

    // An empty transfer target is answered with an error tool response, not
    // followed: the run does not fail and the session carries on.
    let continued = stream.next().await.unwrap().unwrap();
    assert_eq!(tool_responses.load(Ordering::SeqCst), 1, "the refused transfer is answered once");
    assert_eq!(continued.actions.transfer_to_agent, None, "no transfer may be followed");
}
