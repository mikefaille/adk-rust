use std::time::Duration;

use adk_spatial_os::{app_router, server::AppState};

async fn spawn_server() -> (String, tokio::task::JoinHandle<()>) {
    let app = app_router(AppState::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("listener addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("server run");
    });
    (format!("http://{}", addr), handle)
}

fn extract_sse_data(frame: &str) -> Option<String> {
    let mut data_lines = Vec::new();
    for line in frame.lines() {
        if let Some(rest) = line.strip_prefix("data: ") {
            data_lines.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.trim_start().to_string());
        }
    }
    if data_lines.is_empty() {
        None
    } else {
        Some(data_lines.join("\n"))
    }
}

#[tokio::test]
async fn apps_endpoint_returns_catalog() {
    let (base, handle) = spawn_server().await;
    let client = reqwest::Client::new();

    let response = client
        .get(format!("{}/api/os/apps", base))
        .send()
        .await
        .expect("apps response");
    assert!(response.status().is_success());

    let body: serde_json::Value = response.json().await.expect("apps json");
    let apps = body
        .get("apps")
        .and_then(serde_json::Value::as_array)
        .expect("apps array");
    assert!(!apps.is_empty());

    handle.abort();
}

#[tokio::test]
async fn stream_endpoint_uses_event_stream() {
    let (base, handle) = spawn_server().await;
    let client = reqwest::Client::new();

    let create = client
        .post(format!("{}/api/os/session", base))
        .send()
        .await
        .expect("session create");
    let created: serde_json::Value = create.json().await.expect("session json");
    let session_id = created
        .get("session_id")
        .and_then(serde_json::Value::as_str)
        .expect("session_id");

    let response = tokio::time::timeout(
        Duration::from_secs(3),
        client.get(format!("{}/api/os/stream/{}", base, session_id)).send(),
    )
    .await
    .expect("stream timeout")
    .expect("stream response");

    assert!(response.status().is_success());
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_lowercase();
    assert!(content_type.contains("text/event-stream"));

    handle.abort();
}

#[tokio::test]
async fn dangerous_prompt_emits_approval_required() {
    let (base, handle) = spawn_server().await;
    let client = reqwest::Client::new();

    let create = client
        .post(format!("{}/api/os/session", base))
        .send()
        .await
        .expect("session create");
    let created: serde_json::Value = create.json().await.expect("session json");
    let session_id = created
        .get("session_id")
        .and_then(serde_json::Value::as_str)
        .expect("session_id");

    let mut stream_response = client
        .get(format!("{}/api/os/stream/{}", base, session_id))
        .send()
        .await
        .expect("stream response");

    let run = client
        .post(format!("{}/api/os/prompt/{}", base, session_id))
        .json(&serde_json::json!({
            "prompt": "rollback checkout now"
        }))
        .send()
        .await
        .expect("prompt response");
    assert!(run.status().is_success());

    let mut saw_shell_state = false;
    let mut saw_approval = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

    while tokio::time::Instant::now() < deadline {
        let next = tokio::time::timeout(Duration::from_millis(400), stream_response.chunk()).await;
        let Ok(chunk_result) = next else {
            continue;
        };
        let Ok(chunk_opt) = chunk_result else {
            break;
        };
        let Some(chunk) = chunk_opt else {
            break;
        };

        let text = String::from_utf8_lossy(&chunk);
        if text.contains("event: shell_state") {
            saw_shell_state = true;
        }
        if text.contains("event: approval_required") {
            saw_approval = true;
        }
        if saw_shell_state && saw_approval {
            break;
        }
    }

    assert!(saw_shell_state, "expected shell_state event");
    assert!(saw_approval, "expected approval_required event");

    handle.abort();
}

#[tokio::test]
async fn workspace_layout_is_restored_after_prompt_reroute() {
    let (base, handle) = spawn_server().await;
    let client = reqwest::Client::new();

    let create = client
        .post(format!("{}/api/os/session", base))
        .send()
        .await
        .expect("session create");
    let created: serde_json::Value = create.json().await.expect("session json");
    let session_id = created
        .get("session_id")
        .and_then(serde_json::Value::as_str)
        .expect("session_id");

    let mut stream_response = client
        .get(format!("{}/api/os/stream/{}", base, session_id))
        .send()
        .await
        .expect("stream response");

    let layout = serde_json::json!([
        {
            "id": "surface:ops-center",
            "app_id": "ops-center",
            "x": 222,
            "y": 144,
            "w": 520,
            "h": 320,
            "z_index": 29
        }
    ])
    .to_string();

    let layout_event = client
        .post(format!("{}/api/os/event/{}", base, session_id))
        .json(&serde_json::json!({
            "seq": 1,
            "event": {
                "type": "workspace_layout_change",
                "layout": layout
            }
        }))
        .send()
        .await
        .expect("layout event response");
    assert!(layout_event.status().is_success());

    let run = client
        .post(format!("{}/api/os/prompt/{}", base, session_id))
        .json(&serde_json::json!({
            "prompt": "ops incident triage"
        }))
        .send()
        .await
        .expect("prompt response");
    assert!(run.status().is_success());

    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    let mut pending = String::new();
    let mut restored = false;

    while tokio::time::Instant::now() < deadline {
        let next = tokio::time::timeout(Duration::from_millis(500), stream_response.chunk()).await;
        let Ok(chunk_result) = next else {
            continue;
        };
        let Ok(chunk_opt) = chunk_result else {
            break;
        };
        let Some(chunk) = chunk_opt else {
            break;
        };

        pending.push_str(&String::from_utf8_lossy(&chunk).replace('\r', ""));

        while let Some(end) = pending.find("\n\n") {
            let frame = pending[..end].to_string();
            pending = pending[end + 2..].to_string();

            if frame.contains("event: app_surface_ops")
                && frame.contains("\"app_id\":\"ops-center\"")
                && frame.contains("\"x\":222")
                && frame.contains("\"y\":144")
                && frame.contains("\"z_index\":29")
            {
                restored = true;
                break;
            }
        }

        if restored {
            break;
        }
    }

    assert!(restored, "expected restored layout coordinates in app_surface_ops");

    handle.abort();
}

#[tokio::test]
async fn sse_envelope_contract_includes_version_and_shape() {
    let (base, handle) = spawn_server().await;
    let client = reqwest::Client::new();

    let create = client
        .post(format!("{}/api/os/session", base))
        .send()
        .await
        .expect("session create");
    let created: serde_json::Value = create.json().await.expect("session json");
    let session_id = created
        .get("session_id")
        .and_then(serde_json::Value::as_str)
        .expect("session_id")
        .to_string();

    let mut stream_response = client
        .get(format!("{}/api/os/stream/{}", base, session_id))
        .send()
        .await
        .expect("stream response");

    let run = client
        .post(format!("{}/api/os/prompt/{}", base, session_id))
        .json(&serde_json::json!({
            "prompt": "ops status summary"
        }))
        .send()
        .await
        .expect("prompt response");
    assert!(run.status().is_success());

    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    let mut pending = String::new();
    let mut validated = false;

    while tokio::time::Instant::now() < deadline {
        let next = tokio::time::timeout(Duration::from_millis(500), stream_response.chunk()).await;
        let Ok(chunk_result) = next else {
            continue;
        };
        let Ok(chunk_opt) = chunk_result else {
            break;
        };
        let Some(chunk) = chunk_opt else {
            break;
        };

        pending.push_str(&String::from_utf8_lossy(&chunk).replace('\r', ""));

        while let Some(end) = pending.find("\n\n") {
            let frame = pending[..end].to_string();
            pending = pending[end + 2..].to_string();

            if !frame.contains("event: shell_state") {
                continue;
            }
            let Some(data) = extract_sse_data(&frame) else {
                continue;
            };
            let value: serde_json::Value = serde_json::from_str(&data).expect("valid envelope json");

            assert_eq!(value.get("v").and_then(serde_json::Value::as_str), Some("v0"));
            assert!(value.get("seq").and_then(serde_json::Value::as_u64).is_some());
            assert_eq!(
                value.get("session").and_then(serde_json::Value::as_str),
                Some(session_id.as_str())
            );
            assert_eq!(
                value
                    .get("payload")
                    .and_then(|payload| payload.get("kind"))
                    .and_then(serde_json::Value::as_str),
                Some("shell_state")
            );

            validated = true;
            break;
        }

        if validated {
            break;
        }
    }

    assert!(validated, "expected shell_state envelope with contract fields");

    handle.abort();
}

#[tokio::test]
async fn handoff_flow_emits_approval_and_can_activate_target_app() {
    let (base, handle) = spawn_server().await;
    let client = reqwest::Client::new();

    let create = client
        .post(format!("{}/api/os/session", base))
        .send()
        .await
        .expect("session create");
    let created: serde_json::Value = create.json().await.expect("session json");
    let session_id = created
        .get("session_id")
        .and_then(serde_json::Value::as_str)
        .expect("session_id")
        .to_string();

    let mut stream_response = client
        .get(format!("{}/api/os/stream/{}", base, session_id))
        .send()
        .await
        .expect("stream response");

    let command = client
        .post(format!("{}/api/os/event/{}", base, session_id))
        .json(&serde_json::json!({
            "seq": 1,
            "event": {
                "type": "app_command",
                "app_id": "ops-center",
                "command": "handoff mail-agent | include active incident summary"
            }
        }))
        .send()
        .await
        .expect("handoff command response");
    assert!(command.status().is_success());

    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    let mut pending = String::new();
    let mut approval_action_id: Option<String> = None;

    while tokio::time::Instant::now() < deadline {
        let next = tokio::time::timeout(Duration::from_millis(500), stream_response.chunk()).await;
        let Ok(chunk_result) = next else {
            continue;
        };
        let Ok(chunk_opt) = chunk_result else {
            break;
        };
        let Some(chunk) = chunk_opt else {
            break;
        };

        pending.push_str(&String::from_utf8_lossy(&chunk).replace('\r', ""));

        while let Some(end) = pending.find("\n\n") {
            let frame = pending[..end].to_string();
            pending = pending[end + 2..].to_string();
            if !frame.contains("event: approval_required") {
                continue;
            }
            let Some(data) = extract_sse_data(&frame) else {
                continue;
            };
            let value: serde_json::Value = serde_json::from_str(&data).expect("approval envelope");
            approval_action_id = value
                .get("payload")
                .and_then(|payload| payload.get("data"))
                .and_then(|data| data.get("action_id"))
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string);
            if approval_action_id.is_some() {
                break;
            }
        }

        if approval_action_id.is_some() {
            break;
        }
    }

    let action_id = approval_action_id.expect("expected approval action id");

    let approve = client
        .post(format!("{}/api/os/event/{}", base, session_id))
        .json(&serde_json::json!({
            "seq": 2,
            "event": {
                "type": "approval_decision",
                "action_id": action_id,
                "approved": true
            }
        }))
        .send()
        .await
        .expect("approval decision response");
    assert!(approve.status().is_success());

    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    let mut pending = String::new();
    let mut saw_mail_in_shell_state = false;

    while tokio::time::Instant::now() < deadline {
        let next = tokio::time::timeout(Duration::from_millis(500), stream_response.chunk()).await;
        let Ok(chunk_result) = next else {
            continue;
        };
        let Ok(chunk_opt) = chunk_result else {
            break;
        };
        let Some(chunk) = chunk_opt else {
            break;
        };

        pending.push_str(&String::from_utf8_lossy(&chunk).replace('\r', ""));

        while let Some(end) = pending.find("\n\n") {
            let frame = pending[..end].to_string();
            pending = pending[end + 2..].to_string();
            if !frame.contains("event: shell_state") {
                continue;
            }
            let Some(data) = extract_sse_data(&frame) else {
                continue;
            };
            if data.contains("\"mail-agent\"") {
                saw_mail_in_shell_state = true;
                break;
            }
        }

        if saw_mail_in_shell_state {
            break;
        }
    }

    assert!(
        saw_mail_in_shell_state,
        "expected handoff approval to activate target app in shell_state"
    );

    handle.abort();
}

#[tokio::test]
async fn handoff_policy_blocked_route_skips_approval_and_emits_notification() {
    let (base, handle) = spawn_server().await;
    let client = reqwest::Client::new();

    let create = client
        .post(format!("{}/api/os/session", base))
        .send()
        .await
        .expect("session create");
    let created: serde_json::Value = create.json().await.expect("session json");
    let session_id = created
        .get("session_id")
        .and_then(serde_json::Value::as_str)
        .expect("session_id")
        .to_string();

    let mut stream_response = client
        .get(format!("{}/api/os/stream/{}", base, session_id))
        .send()
        .await
        .expect("stream response");

    let command = client
        .post(format!("{}/api/os/event/{}", base, session_id))
        .json(&serde_json::json!({
            "seq": 1,
            "event": {
                "type": "app_command",
                "app_id": "mail-agent",
                "command": "handoff calendar-agent | share latest customer impact summary"
            }
        }))
        .send()
        .await
        .expect("handoff command response");
    assert!(command.status().is_success());

    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    let mut pending = String::new();
    let mut saw_approval = false;
    let mut saw_policy_block = false;

    while tokio::time::Instant::now() < deadline {
        let next = tokio::time::timeout(Duration::from_millis(500), stream_response.chunk()).await;
        let Ok(chunk_result) = next else {
            continue;
        };
        let Ok(chunk_opt) = chunk_result else {
            break;
        };
        let Some(chunk) = chunk_opt else {
            break;
        };

        pending.push_str(&String::from_utf8_lossy(&chunk).replace('\r', ""));

        while let Some(end) = pending.find("\n\n") {
            let frame = pending[..end].to_string();
            pending = pending[end + 2..].to_string();

            if frame.contains("event: approval_required") {
                saw_approval = true;
            }
            if frame.contains("event: notification")
                && frame.to_lowercase().contains("handoff blocked by allowlist")
            {
                saw_policy_block = true;
            }
        }

        if saw_policy_block {
            break;
        }
    }

    assert!(
        !saw_approval,
        "blocked handoff should not emit approval_required"
    );
    assert!(
        saw_policy_block,
        "expected policy-block notification for unlisted handoff route"
    );

    handle.abort();
}
