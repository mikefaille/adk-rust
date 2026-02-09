use adk_studio::{AppState, FileStorage, api_routes};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use std::path::PathBuf;
use tower::ServiceExt;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

#[tokio::test]
async fn test_cors_origins() {
    let temp_dir =
        std::env::temp_dir().join(format!("adk-studio-test-cors-{}", uuid::Uuid::new_v4()));
    let storage = FileStorage::new(temp_dir.clone()).await.unwrap();
    let state = AppState::new(storage);

    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin, _| {
            let origin_str = origin.to_str().unwrap_or("");
            origin_str.starts_with("http://localhost:")
                || origin_str.starts_with("https://localhost:")
                || origin_str == "http://localhost"
                || origin_str == "https://localhost"
                || origin_str.starts_with("http://127.0.0.1:")
                || origin_str.starts_with("https://127.0.0.1:")
                || origin_str == "http://127.0.0.1"
                || origin_str == "https://127.0.0.1"
                || origin_str.starts_with("http://[::1]:")
                || origin_str.starts_with("https://[::1]:")
                || origin_str == "http://[::1]"
                || origin_str == "https://[::1]"
        }))
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new().nest("/api", api_routes()).layer(cors).with_state(state);

    // 1. Allowed origin: localhost
    let req = Request::builder()
        .method("OPTIONS")
        .uri("/api/projects")
        .header(header::ORIGIN, "http://localhost:3000")
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
        .body(Body::empty())
        .unwrap();

    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(),
        "http://localhost:3000"
    );

    // 2. Allowed origin: 127.0.0.1
    let req = Request::builder()
        .method("OPTIONS")
        .uri("/api/projects")
        .header(header::ORIGIN, "http://127.0.0.1:8080")
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
        .body(Body::empty())
        .unwrap();

    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(),
        "http://127.0.0.1:8080"
    );

    // 3. Allowed origin: [::1]
    let req = Request::builder()
        .method("OPTIONS")
        .uri("/api/projects")
        .header(header::ORIGIN, "http://[::1]:8080")
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
        .body(Body::empty())
        .unwrap();

    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(),
        "http://[::1]:8080"
    );

    // 4. Disallowed origin: malicious.com
    let req = Request::builder()
        .method("OPTIONS")
        .uri("/api/projects")
        .header(header::ORIGIN, "http://malicious.com")
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
        .body(Body::empty())
        .unwrap();

    let response = app.clone().oneshot(req).await.unwrap();
    // CORS middleware returns OK but without the ALLOW_ORIGIN header if rejected
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none());

    // Clean up
    let _ = std::fs::remove_dir_all(temp_dir);
}
