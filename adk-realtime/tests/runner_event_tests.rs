use adk_realtime::events::{ClientEvent, ServerEvent, ToolResponse};
use adk_realtime::{
    BoxedModel, RealtimeConfig, RealtimeError, RealtimeModel, RealtimeRunner,
    audio::AudioChunk,
    audio::AudioFormat,
    runner::EventHandler,
    session::{BoxedSession, ContextMutationOutcome, RealtimeSession},
};
use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;
use tokio::sync::mpsc;

struct FakeSession {
    events: Mutex<mpsc::Receiver<Result<ServerEvent, RealtimeError>>>,
}

#[async_trait]
impl RealtimeSession for FakeSession {
    fn session_id(&self) -> &str {
        "fake-session"
    }

    fn is_connected(&self) -> bool {
        true
    }

    async fn send_audio(&self, _audio: &AudioChunk) -> Result<(), RealtimeError> {
        Ok(())
    }

    async fn send_audio_base64(&self, _audio_base64: &str) -> Result<(), RealtimeError> {
        Ok(())
    }

    async fn send_text(&self, _text: &str) -> Result<(), RealtimeError> {
        Ok(())
    }

    async fn send_tool_response(&self, _response: ToolResponse) -> Result<(), RealtimeError> {
        Ok(())
    }

    async fn commit_audio(&self) -> Result<(), RealtimeError> {
        Ok(())
    }

    async fn clear_audio(&self) -> Result<(), RealtimeError> {
        Ok(())
    }

    async fn create_response(&self) -> Result<(), RealtimeError> {
        Ok(())
    }

    async fn interrupt(&self) -> Result<(), RealtimeError> {
        Ok(())
    }

    async fn send_event(&self, _event: ClientEvent) -> Result<(), RealtimeError> {
        Ok(())
    }

    async fn next_event(&self) -> Option<Result<ServerEvent, RealtimeError>> {
        let mut events = self.events.lock().await;
        events.recv().await
    }

    fn events(
        &self,
    ) -> Pin<Box<dyn Stream<Item = Result<ServerEvent, RealtimeError>> + Send + '_>> {
        unimplemented!()
    }

    async fn close(&self) -> Result<(), RealtimeError> {
        Ok(())
    }

    async fn mutate_context(
        &self,
        _config: RealtimeConfig,
    ) -> Result<ContextMutationOutcome, RealtimeError> {
        Ok(ContextMutationOutcome::Applied)
    }
}

struct FakeModel {
    events_tx: Mutex<Option<mpsc::Sender<Result<ServerEvent, RealtimeError>>>>,
}

#[async_trait]
impl RealtimeModel for FakeModel {
    fn provider(&self) -> &str {
        "fake"
    }

    fn model_id(&self) -> &str {
        "fake-model"
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

    async fn connect(&self, _config: RealtimeConfig) -> Result<BoxedSession, RealtimeError> {
        let (tx, rx) = mpsc::channel(10);
        let mut locked_tx = self.events_tx.lock().await;
        *locked_tx = Some(tx);
        Ok(Box::new(FakeSession { events: Mutex::new(rx) }))
    }
}

struct TrackingEventHandler {
    speech_started_called: Arc<AtomicBool>,
}

#[async_trait]
impl EventHandler for TrackingEventHandler {
    async fn on_speech_started(&self, _audio_start_ms: u64) -> Result<(), RealtimeError> {
        self.speech_started_called.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn runner_run_dispatches_events_to_handler() {
    let model = Arc::new(FakeModel { events_tx: Mutex::new(None) });
    let handler = TrackingEventHandler { speech_started_called: Arc::new(AtomicBool::new(false)) };
    let called = handler.speech_started_called.clone();

    let runner = RealtimeRunner::builder()
        .model(model.clone() as BoxedModel)
        .event_handler(handler)
        .build()
        .unwrap();

    runner.connect().await.unwrap();

    let tx = model.events_tx.lock().await.take().unwrap();
    tx.send(Ok(ServerEvent::SpeechStarted {
        audio_start_ms: 100,
        event_id: "test-id".to_string(),
    }))
    .await
    .unwrap();

    // Send EOF to stop the runner loop
    drop(tx);

    runner.run().await.unwrap();

    assert!(called.load(Ordering::SeqCst));
}

#[tokio::test]
async fn runner_next_event_does_not_invoke_handler() {
    let model = Arc::new(FakeModel { events_tx: Mutex::new(None) });
    let handler = TrackingEventHandler { speech_started_called: Arc::new(AtomicBool::new(false)) };
    let called = handler.speech_started_called.clone();

    let runner = RealtimeRunner::builder()
        .model(model.clone() as BoxedModel)
        .event_handler(handler)
        .build()
        .unwrap();

    runner.connect().await.unwrap();

    let tx = model.events_tx.lock().await.take().unwrap();
    tx.send(Ok(ServerEvent::SpeechStarted {
        audio_start_ms: 100,
        event_id: "test-id".to_string(),
    }))
    .await
    .unwrap();

    let event = runner.next_event().await.unwrap().unwrap();
    assert!(matches!(event, ServerEvent::SpeechStarted { audio_start_ms: 100, .. }));

    // Handler should not be called in manual polling mode
    assert!(!called.load(Ordering::SeqCst));
}
