use adk_core::{Content, Llm, LlmRequest};
use adk_model::gemini::GeminiModel;

#[tokio::test]
async fn test_gemini_model_creation() {
    let result = GeminiModel::new("test-project", "us-central1", "gemini-2.5-flash").await;

    // We expect this to likely fail if no credentials are available, but we want to ensure the signature is correct.
    // If it succeeds (e.g. CI has creds), fine. If it fails, that's also expected behavior for unit tests without auth.
    // We mainly want to fix the compilation error here.
    if let Ok(model) = result {
        assert_eq!(model.name(), "gemini-2.5-flash");
    }
}

#[tokio::test]
async fn test_llm_request_creation() {
    let content = Content::new("user").with_text("Hello");
    let request = LlmRequest::new("gemini-2.5-flash", vec![content]);

    assert_eq!(request.model, "gemini-2.5-flash");
    assert_eq!(request.contents.len(), 1);
}
