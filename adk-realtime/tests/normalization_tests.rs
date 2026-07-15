use adk_realtime::events::ServerEvent;
use serde_json::json;

#[cfg(feature = "openai")]
mod openai_tests {
    use super::*;
    use adk_realtime::error::RealtimeError;
    use adk_realtime::openai::OpenAIRealtimeSession;

    #[tokio::test]
    async fn test_openai_argument_normalization() {
        let raw_event = json!({
            "type": "response.function_call_arguments.done",
            "event_id": "evt_1",
            "response_id": "resp_1",
            "item_id": "item_1",
            "output_index": 0,
            "call_id": "call_1",
            "name": "test_tool",
            "arguments": "{\"key\": \"value\"}"
        })
        .to_string();

        let event = OpenAIRealtimeSession::translate_event(&raw_event).unwrap();
        if let ServerEvent::FunctionCallDone { arguments, .. } = event {
            assert_eq!(arguments, json!({"key": "value"}));
        } else {
            panic!("Expected FunctionCallDone, got {:?}", event);
        }
    }

    #[tokio::test]
    async fn test_openai_malformed_argument_normalization() {
        let raw_event = json!({
            "type": "response.function_call_arguments.done",
            "event_id": "evt_1",
            "response_id": "resp_1",
            "item_id": "item_1",
            "output_index": 0,
            "call_id": "call_1",
            "name": "test_tool",
            "arguments": "{\"key\": \"value\"" // Malformed JSON
        })
        .to_string();

        let result = OpenAIRealtimeSession::translate_event(&raw_event);
        match result {
            Err(RealtimeError::Protocol(msg)) => {
                assert!(msg.contains("malformed function arguments"));
            }
            _ => panic!("Expected Protocol error, got {:?}", result),
        }
    }

    #[tokio::test]
    async fn test_openai_non_object_argument_rejection() {
        let raw_event = json!({
            "type": "response.function_call_arguments.done",
            "event_id": "evt_1",
            "response_id": "resp_1",
            "item_id": "item_1",
            "output_index": 0,
            "call_id": "call_1",
            "name": "test_tool",
            "arguments": "\"just a string\""
        })
        .to_string();

        let result = OpenAIRealtimeSession::translate_event(&raw_event);
        match result {
            Err(RealtimeError::Protocol(msg)) => {
                assert!(msg.contains("malformed function arguments"));
            }
            _ => panic!("Expected Protocol error, got {:?}", result),
        }
    }
}

#[cfg(feature = "gemini")]
mod gemini_tests {
    use super::*;
    use adk_realtime::gemini::GeminiRealtimeSession;

    #[tokio::test]
    async fn test_gemini_argument_normalization() {
        let raw_event = json!({
            "toolCall": {
                "functionCalls": [{
                    "name": "test_tool",
                    "id": "call_1",
                    "args": {"key": "value"}
                }]
            }
        })
        .to_string();

        let events = GeminiRealtimeSession::translate_event_static(&raw_event).unwrap();
        assert_eq!(events.len(), 1);
        if let ServerEvent::FunctionCallDone { arguments, .. } = &events[0] {
            assert_eq!(arguments, &json!({"key": "value"}));
        } else {
            panic!("Expected FunctionCallDone");
        }
    }

    #[tokio::test]
    async fn test_gemini_non_object_argument_rejection() {
        let raw_event = json!({
            "toolCall": {
                "functionCalls": [{
                    "name": "test_tool",
                    "id": "call_1",
                    "args": "not_an_object"
                }]
            }
        })
        .to_string();

        let result = GeminiRealtimeSession::translate_event_static(&raw_event);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("malformed Gemini tool call"));
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

    #[derive(Debug)]
    struct MockModel;
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
            Ok(Box::new(MockSession))
        }
    }

    struct MockSession;
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

    struct MockInvocationContext {
        content: Content,
        agent: Arc<dyn Agent>,
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
            unimplemented!()
        }
        fn run_config(&self) -> &RunConfig {
            static R: once_cell::sync::Lazy<RunConfig> =
                once_cell::sync::Lazy::new(RunConfig::default);
            &R
        }
        fn end_invocation(&self) {}
        fn ended(&self) -> bool {
            false
        }
    }

    let sub_agent = RealtimeAgent::builder("sub").model(Arc::new(MockModel)).build().unwrap();
    let agent = RealtimeAgent::builder("test")
        .model(Arc::new(MockModel))
        .sub_agent(Arc::new(sub_agent))
        .build()
        .unwrap();

    let ctx = MockInvocationContext {
        content: Content { role: "user".into(), parts: vec![] },
        agent: Arc::new(RealtimeAgent::builder("test").model(Arc::new(MockModel)).build().unwrap()),
    };
    let mut stream = agent.run(Arc::new(ctx)).await.unwrap();

    // First event is session started
    let _ = stream.next().await.unwrap().unwrap();

    // Second event should be the tool response event with the error because of empty agent_name
    let result = stream.next().await.unwrap().unwrap();
    if let Some(content) = result.llm_response.content {
        let mut found_error = false;
        for part in content.parts {
            if let adk_core::Part::FunctionResponse { function_response, .. } = part {
                if let Some(err) = function_response.response.get("error") {
                    assert!(
                        err.as_str().unwrap().contains("Transfer target (agent_name) is missing")
                    );
                    found_error = true;
                }
            }
        }
        assert!(found_error, "Should have found error in tool response");
    } else {
        panic!("Expected tool response content");
    }
}
