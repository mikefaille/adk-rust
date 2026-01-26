use adk_gemini::GeminiProvider;
use adk_gemini::{GenerateContentRequest, Content, Part, part};
use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();

    let project_id = env::var("GOOGLE_PROJECT_ID").expect("GOOGLE_PROJECT_ID not set");
    let location = env::var("GOOGLE_LOCATION").unwrap_or_else(|_| "us-central1".to_string());

    let provider = GeminiProvider::new(&project_id, &location).await?;

    let req = GenerateContentRequest {
        model: "gemini-2.5-flash".to_string(),
        contents: vec![Content {
            role: "user".to_string(),
            parts: vec![Part {
                data: Some(part::Data::Text("Hello, world!".to_string())),
                ..Default::default()
            }],
        }],
        ..Default::default()
    };

    let response = provider.generate_content(req).await?;
    println!("Response: {:?}", response);

    Ok(())
}
