#![cfg(feature = "gemini")]

//! Managed supervisor rebuild proofs against a scripted local WebSocket server.
//!
//! Drives the public [`RealtimeRunner`](adk_realtime::RealtimeRunner) (the
//! supervisor is `pub(crate)`) over a mock Gemini Live endpoint. Connection 0
//! follows a fault script while later connections answer `setup` with
//! `setupComplete`, so a rebuild must open a second TCP connection to succeed.

use adk_realtime::RealtimeRunner;
use adk_realtime::events::ServerEvent;
use adk_realtime::gemini::{GeminiLiveBackend, GeminiRealtimeModel};
use adk_realtime::recovery::RecoveryPolicy;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

type WsStream = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

async fn spawn_scripted_server<F, Fut>(
    handler: F,
) -> (SocketAddr, Arc<AtomicUsize>, tokio::task::JoinHandle<()>)
where
    F: Fn(usize, WsStream) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let conns = Arc::new(AtomicUsize::new(0));
    let handler = Arc::new(handler);
    let conns_clone = Arc::clone(&conns);
    let handle = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let Ok(ws) = accept_async(stream).await else {
                continue;
            };
            let idx = conns_clone.fetch_add(1, Ordering::SeqCst);
            let handler = Arc::clone(&handler);
            tokio::spawn(async move { handler(idx, ws).await });
        }
    });
    (addr, conns, handle)
}

async fn send_json(ws: &mut WsStream, value: serde_json::Value) {
    ws.send(Message::Text(value.to_string().into())).await.expect("mock server sends frame");
}

async fn read_setup(ws: &mut WsStream) -> serde_json::Value {
    let frame = tokio::time::timeout(Duration::from_secs(10), ws.next())
        .await
        .expect("client sends setup promptly")
        .expect("setup stream stays open")
        .expect("setup frame receives");
    match frame {
        Message::Text(text) => serde_json::from_str(&text).expect("setup parses as json"),
        other => panic!("expected text setup frame, got {other:?}"),
    }
}

fn test_runner(addr: SocketAddr) -> RealtimeRunner {
    let backend = GeminiLiveBackend::studio("test-key").with_endpoint_url(format!("ws://{addr}"));
    let model = Arc::new(GeminiRealtimeModel::new(backend, "models/gemini-live"));
    RealtimeRunner::builder()
        .model(model)
        .recovery_policy(RecoveryPolicy::default().with_deadline(Duration::from_secs(5)))
        .build()
        .expect("runner builds")
}

#[tokio::test]
async fn test_goaway_planned_rotation_rebuilds_new_generation() {
    let (addr, conns, server) = spawn_scripted_server(|idx, mut ws| async move {
        let setup = read_setup(&mut ws).await;
        assert!(setup.get("setup").is_some(), "conn {idx}: expected setup frame");
        send_json(&mut ws, json!({ "setupComplete": {} })).await;
        if idx == 0 {
            send_json(&mut ws, json!({ "goAway": { "timeLeft": "30s" } })).await;
        }
        // Hold the connection open; the client closes conn 0 after publishing gen 1.
        while let Some(msg) = ws.next().await {
            if matches!(msg, Ok(Message::Close(_)) | Err(_)) {
                break;
            }
        }
    })
    .await;

    let runner = test_runner(addr);
    runner.connect().await.expect("initial connect succeeds");
    let gen0_session = runner.session_id().await.expect("gen 0 session id");

    let ev1 = tokio::time::timeout(Duration::from_secs(10), runner.next_event())
        .await
        .expect("first event arrives")
        .expect("stream stays open")
        .expect("setupComplete parses");
    assert!(
        matches!(ev1, ServerEvent::SessionCreated { .. }),
        "expected SessionCreated, got {ev1:?}"
    );

    let ev2 = tokio::time::timeout(Duration::from_secs(10), runner.next_event())
        .await
        .expect("planned rotation arrives")
        .expect("stream stays open")
        .expect("goAway parses");
    match ev2 {
        ServerEvent::PlannedRotation { time_left } => {
            assert_eq!(time_left.as_deref(), Some("30s"));
        }
        other => panic!("expected PlannedRotation, got {other:?}"),
    }

    // The planned replacement runs in the background; wait for gen 1 publication.
    let mut gen_rx = runner.subscribe_generation();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if *gen_rx.borrow() == 1 {
                break;
            }
            gen_rx.changed().await.expect("generation watcher stays live");
        }
    })
    .await
    .expect("generation 1 publishes");

    assert_eq!(conns.load(Ordering::SeqCst), 2, "rebuild must open a second connection");
    let gen1_session = runner.session_id().await.expect("gen 1 session id");
    assert_ne!(gen0_session, gen1_session, "generation 1 is a new provider session");
    assert!(runner.is_connected().await);

    runner.close().await.expect("close succeeds");
    server.abort();
}

#[tokio::test]
async fn test_setup_refusal_close_is_fatal_without_rebuild() {
    use tokio_tungstenite::tungstenite::protocol::CloseFrame;
    use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

    let (addr, conns, server) = spawn_scripted_server(|_idx, mut ws| async move {
        let _ = ws
            .send(Message::Close(Some(CloseFrame {
                code: CloseCode::Policy,
                reason: "setup refused".into(),
            })))
            .await;
        let _ = ws.flush().await;
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            while ws.next().await.is_some() {}
        })
        .await;
    })
    .await;

    let runner = test_runner(addr);
    runner.connect().await.expect("connect queues setup");
    let outcome = tokio::time::timeout(Duration::from_secs(10), runner.next_event())
        .await
        .expect("terminal outcome arrives");
    assert!(outcome.is_none(), "fatal refusal ends the stream, got {outcome:?}");

    // Settle: a recoverable path would open its second connection within milliseconds.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(conns.load(Ordering::SeqCst), 1, "fatal refusal must not rebuild");
    assert!(!runner.is_connected().await);
    let gen_rx = runner.subscribe_generation();
    assert_eq!(*gen_rx.borrow(), 0, "generation must not advance");

    runner.close().await.expect("close succeeds");
    server.abort();
}
