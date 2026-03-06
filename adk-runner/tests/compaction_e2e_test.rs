//! End-to-end compaction test using InMemorySessionService.
//!
//! Verifies the full flow: multiple invocations → compaction triggers →
//! compacted history is used by subsequent invocations.

use adk_core::{
    Agent, BaseEventsSummarizer, Content, Event, EventActions, EventCompaction, EventStream,
    EventsCompactionConfig, InvocationContext, Part, Result, Role,
};
use adk_core::types::{SessionId, UserId};
use adk_runner::{Runner, RunnerConfig};
use adk_session::{CreateRequest, GetRequest, InMemorySessionService, SessionService};
use async_trait::async_trait;
use futures::StreamExt;
use std::sync::{Arc, Mutex};

/// Agent that echoes back a response and records the conversation history it received.
struct HistoryCapturingAgent {
    name: String,
    /// Stores the conversation history seen by the agent on each run.
    captured_histories: Arc<Mutex<Vec<Vec<Content>>>>,
}

#[async_trait]
impl Agent for HistoryCapturingAgent {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run(&self, ctx: Arc<dyn InvocationContext>) -> Result<EventStream> {
        // Record history received from context
        {
            let mut histories = self.captured_histories.lock().unwrap();
            histories.push(ctx.conversation_history());
        }

        let invocation_id = ctx.invocation_id().clone();
        let mut event = Event::new(invocation_id).with_author(&self.name);

        // First message in our test triggers compaction
        if self.captured_histories.lock().unwrap().len() == 1 {
            event.set_content(Content::new(Role::Model).with_text("Agent response"));
            event.actions.compaction = Some(EventCompaction {
                truncate_before_id: event.id.clone(),
                summary: Some("Summary of conversation".to_string()),
                compacted_content: Content::new(Role::Model).with_text("Summary of conversation"),
                end_timestamp: chrono::Utc::now(),
            });
        } else {
            event.set_content(Content::new(Role::Model).with_text("Second response"));
        }

        let s = futures::stream::once(async move { Ok(event) });
        Ok(Box::pin(s))
    }
}

/// Simple summarizer that just returns a fixed string
struct MockSummarizer {
    summary_text: String,
}

#[async_trait]
impl BaseEventsSummarizer for MockSummarizer {
    async fn summarize_events(&self, _events: &[Event]) -> Result<Option<Content>> {
        Ok(Some(Content::new(Role::Model).with_text(&self.summary_text)))
    }
}

#[tokio::test]
async fn test_e2e_compaction_flow() {
    let captured_histories = Arc::new(Mutex::new(Vec::new()));
    let agent = Arc::new(HistoryCapturingAgent {
        name: "test_agent".to_string(),
        captured_histories: captured_histories.clone(),
    });

    let session_service = Arc::new(InMemorySessionService::new());
    
    // Create session first
    session_service
        .create(CreateRequest {
            app_name: "test_app".to_string(),
            user_id: UserId::new("user-1").unwrap(),
            session_id: Some(SessionId::new("sess-e2e").unwrap()),
            state: Default::default(),
        })
        .await
        .unwrap();

    let runner = Runner::new(RunnerConfig {
        app_name: "test_app".to_string(),
        agent,
        session_service: session_service.clone(),
        artifact_service: None,
        memory_service: None,
        plugin_manager: None,
        run_config: None,
        compaction_config: Some(EventsCompactionConfig {
            summarizer: Arc::new(MockSummarizer {
                summary_text: "Summary of conversation".to_string(),
            }),
            trigger_event_count: 5, // Not used in this manual test
        }),
        context_cache_config: None,
        cache_capable: None,
    })
    .unwrap();

    // 1. First invocation - should return event with compaction instruction
    let content1 = Content::new(Role::User).with_text("Hello");
    let mut stream1 = runner
        .run(UserId::new("user-1").unwrap(), SessionId::new("sess-e2e").unwrap(), content1)
        .await
        .unwrap();

    // Consume stream to ensure event is processed and stored
    while let Some(res) = stream1.next().await {
        res.unwrap();
    }

    // 2. Second invocation - should see COMPACTED history
    let content2 = Content::new(Role::User).with_text("How are you?");
    let mut stream2 = runner
        .run(UserId::new("user-1").unwrap(), SessionId::new("sess-e2e").unwrap(), content2)
        .await
        .unwrap();

    while let Some(res) = stream2.next().await {
        res.unwrap();
    }

    // 3. Verify what the agent saw
    let histories = captured_histories.lock().unwrap();
    assert_eq!(histories.len(), 2);

    // First run saw: [User("Hello")]
    assert_eq!(histories[0].len(), 1);
    assert_eq!(histories[0][0].text(), "Hello");

    // Second run should see: [Model("Summary of conversation"), User("How are you?")]
    // because the first run's response included a compaction instruction for EVERYTHING up to that point.
    assert_eq!(histories[1].len(), 2);
    assert_eq!(histories[1][0].text(), "Summary of conversation");
    assert_eq!(histories[1][1].text(), "How are you?");
}

#[tokio::test]
async fn test_compaction_state_preservation() {
    let session_service = Arc::new(InMemorySessionService::new());

    let req = CreateRequest {
        app_name: "test_app".to_string(),
        user_id: UserId::new("user-1").unwrap(),
        session_id: Some(SessionId::new("sess-e2e").unwrap()),
        state: Default::default(),
    };
    session_service.create(req).await.unwrap();

    // Manually add a compaction event
    let compaction = EventCompaction {
        truncate_before_id: "none".to_string(),
        summary: Some("Summary".to_string()),
        compacted_content: Content::new(Role::Model).with_text("Summary of conversation"),
        end_timestamp: chrono::Utc::now(),
    };

    let mut event = Event::new(InvocationId::new("inv-1").unwrap());
    event.actions.compaction = Some(compaction);
    session_service.append_event(&SessionId::new("sess-e2e").unwrap(), event).await.unwrap();

    // Get session and verify restored compaction state
    let restored = session_service
        .get(GetRequest {
            app_name: "test_app".to_string(),
            user_id: UserId::new("user-1").unwrap(),
            session_id: SessionId::new("sess-e2e").unwrap(),
            num_recent_events: None,
            after: None,
        })
        .await
        .unwrap();

    assert_eq!(restored.compacted_content().unwrap().text(), "Summary of conversation");
    assert_eq!(restored.compacted_content().unwrap().role, Role::Model);
}
