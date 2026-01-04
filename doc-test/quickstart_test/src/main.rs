use adk_rust::prelude::*;
use adk_rust::Launcher;
use std::sync::Arc;

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    
    let api_key = std::env::var("GOOGLE_API_KEY")
        .expect("GOOGLE_API_KEY environment variable not set");

    let model = GeminiModel::new(&api_key, "gemini-2.5-flash")?;

    // Build agent with Google Search tool
    let agent = LlmAgentBuilder::new("search_assistant")
        .description("An assistant that can search the web")
        .instruction("You are a helpful assistant. Use the search tool to find current information when needed.")
        .model(Arc::new(model))
        .tool(Arc::new(GoogleSearchTool::new()))  // Add search capability
        .build()?;

    Launcher::new(Arc::new(agent)).run().await?;

    Ok(())
}
