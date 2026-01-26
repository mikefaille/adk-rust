use adk_core::{Content, Llm, LlmRequest, Part};
use adk_model::gemini::{GeminiModel, streaming::aggregate_stream};

fn get_config() -> Option<(String, String)> {
    let project_id = std::env::var("GOOGLE_PROJECT_ID").ok()?;
    let location = std::env::var("GOOGLE_LOCATION").unwrap_or_else(|_| "us-central1".to_string());
    Some((project_id, location))
}

#[tokio::test]
#[ignore]
async fn test_stream_aggregation() {
    let (project_id, location) = match get_config() {
        Some(c) => c,
        None => {
            println!("Skipping test: GOOGLE_PROJECT_ID not set");
            return;
        }
    };

    let model = GeminiModel::new(project_id, location, "gemini-2.5-flash").await.unwrap();
    let content = Content::new("user").with_text("Count from 1 to 5");
    let request = LlmRequest::new("gemini-2.5-flash", vec![content]);

    let stream = model.generate_content(request, true).await.unwrap();
    let aggregated = aggregate_stream(stream).await.unwrap();

    assert!(aggregated.content.is_some());
    assert!(!aggregated.partial);
    assert!(aggregated.turn_complete);

    let content = aggregated.content.unwrap();
    let part = content.parts.first().unwrap();
    if let Part::Text { text } = part {
        assert!(!text.is_empty());
        println!("Aggregated: {}", text);
    }
}
