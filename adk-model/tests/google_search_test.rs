use adk_core::{Content, Llm, LlmRequest};
use adk_model::gemini::GeminiModel;
use futures::StreamExt;
use serde_json::json;

fn get_config() -> Option<(String, String)> {
    let project_id = std::env::var("GOOGLE_PROJECT_ID").ok()?;
    let location = std::env::var("GOOGLE_LOCATION").unwrap_or_else(|_| "us-central1".to_string());
    Some((project_id, location))
}

#[tokio::test]
#[ignore]
async fn test_google_search_zavora() {
    let (project_id, location) = match get_config() {
        Some(c) => c,
        None => {
            println!("Skipping test: GOOGLE_PROJECT_ID not set");
            return;
        }
    };

    let model = GeminiModel::new(project_id, location, "gemini-2.5-flash").await.unwrap();

    let content =
        Content::new("user").with_text("Search for information about Zavora Technologies");
    let mut request = LlmRequest::new("gemini-2.5-flash", vec![content]);

    // Add Google Search tool
    let google_search_tool = json!({
        "googleSearch": {}
    });
    request.tools.insert("google_search".to_string(), google_search_tool);

    let mut stream = model.generate_content(request, false).await.unwrap();
    let response = stream.next().await.unwrap().unwrap();

    assert!(response.content.is_some());
    let content = response.content.unwrap();
    let part = content.parts.first().unwrap();

    if let adk_core::Part::Text { text } = part {
        println!("Response: {}", text);
        assert!(!text.is_empty());
        // Should contain information from search
        assert!(text.len() > 50);
    }
}
