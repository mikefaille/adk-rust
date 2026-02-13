//! Roadmap example: adk-gemini SDK surface for v1 + Vertex.
//!
//! Run with:
//!   cargo run --example roadmap_gemini_sdk
//!
//! Modes (ROADMAP_SDK_MODE):
//!   v1_api_key | vertex_api_key | vertex_adc | vertex_service_account | vertex_wif
//!
//! Optional:
//!   ROADMAP_RUN_EMBED=1 (default) to also run embedContent
//!   ROADMAP_PROMPT

use adk_gemini::{Gemini, GeminiBuilder, Model, TaskType};
use anyhow::{Context, Result, bail};
use std::{env, fs};

#[derive(Debug, Clone, Copy)]
enum SdkMode {
    V1ApiKey,
    VertexApiKey,
    VertexAdc,
    VertexServiceAccount,
    VertexWif,
}

impl SdkMode {
    fn from_env() -> Self {
        match env::var("ROADMAP_SDK_MODE")
            .unwrap_or_else(|_| "v1_api_key".to_string())
            .to_lowercase()
            .as_str()
        {
            "vertex_api_key" => Self::VertexApiKey,
            "vertex_adc" => Self::VertexAdc,
            "vertex_service_account" => Self::VertexServiceAccount,
            "vertex_wif" => Self::VertexWif,
            _ => Self::V1ApiKey,
        }
    }
}

fn google_api_key() -> Option<String> {
    env::var("GOOGLE_API_KEY").ok().or_else(|| env::var("GEMINI_API_KEY").ok())
}

fn project_id() -> Result<String> {
    env::var("GOOGLE_PROJECT_ID")
        .or_else(|_| env::var("GOOGLE_CLOUD_PROJECT"))
        .context("set GOOGLE_PROJECT_ID (or GOOGLE_CLOUD_PROJECT)")
}

fn cloud_location() -> String {
    env::var("GOOGLE_CLOUD_LOCATION").unwrap_or_else(|_| "us-central1".to_string())
}

fn read_json_value(json_var: &str, path_var: &str) -> Result<String> {
    if let Ok(value) = env::var(json_var) {
        return Ok(value);
    }
    let path = env::var(path_var).with_context(|| format!("set {} or {}", json_var, path_var))?;
    fs::read_to_string(&path).with_context(|| format!("failed to read {}", path))
}

fn build_generate_client(mode: SdkMode, model_str: String) -> Result<Gemini> {
    let location = cloud_location();
    let model = Model::from(model_str);
    match mode {
        SdkMode::V1ApiKey => {
            let Some(api_key) = google_api_key() else {
                bail!("set GOOGLE_API_KEY (or GEMINI_API_KEY) for ROADMAP_SDK_MODE=v1_api_key");
            };
            Ok(GeminiBuilder::new(api_key).with_model(model).build()?)
        }
        SdkMode::VertexApiKey => {
            let Some(api_key) = google_api_key() else {
                bail!("set GOOGLE_API_KEY (or GEMINI_API_KEY) for ROADMAP_SDK_MODE=vertex_api_key");
            };
            let project_id = project_id()?;
            Ok(GeminiBuilder::new(api_key)
                .with_google_cloud(&project_id, &location)
                .with_model(model)
                .build()?)
        }
        SdkMode::VertexAdc => {
            let project_id = project_id()?;
            Ok(GeminiBuilder::new_without_api_key()
                .with_google_cloud_adc()?
                .with_google_cloud(&project_id, &location)
                .with_model(model)
                .build()?)
        }
        SdkMode::VertexServiceAccount => {
            let service_account_json =
                read_json_value("GOOGLE_SERVICE_ACCOUNT_JSON", "GOOGLE_SERVICE_ACCOUNT_PATH")?;
            Ok(GeminiBuilder::new_without_api_key()
                .with_service_account_json(&service_account_json)?
                // Note: we still need project/location for Vertex
                .with_google_cloud(&project_id()?, &location)
                .with_model(model)
                .build()?)
        }
        SdkMode::VertexWif => {
            let wif_json = read_json_value("GOOGLE_WIF_JSON", "GOOGLE_WIF_PATH")?;
            let project_id = project_id()?;
            Ok(GeminiBuilder::new_without_api_key()
                .with_google_cloud_wif_json(&wif_json)?
                .with_google_cloud(&project_id, &location)
                .with_model(model)
                .build()?)
        }
    }
}

fn build_embedding_client(mode: SdkMode) -> Result<Gemini> {
    let location = cloud_location();
    let model = Model::TextEmbedding004;
    match mode {
        SdkMode::V1ApiKey => {
            let Some(api_key) = google_api_key() else {
                bail!("set GOOGLE_API_KEY (or GEMINI_API_KEY) for ROADMAP_SDK_MODE=v1_api_key");
            };
            Ok(GeminiBuilder::new(api_key).with_model(model).build()?)
        }
        SdkMode::VertexApiKey => {
            let Some(api_key) = google_api_key() else {
                bail!("set GOOGLE_API_KEY (or GEMINI_API_KEY) for ROADMAP_SDK_MODE=vertex_api_key");
            };
            let project_id = project_id()?;
            Ok(GeminiBuilder::new(api_key)
                .with_google_cloud(&project_id, &location)
                .with_model(model)
                .build()?)
        }
        SdkMode::VertexAdc => {
            let project_id = project_id()?;
            Ok(GeminiBuilder::new_without_api_key()
                .with_google_cloud_adc()?
                .with_google_cloud(&project_id, &location)
                .with_model(model)
                .build()?)
        }
        SdkMode::VertexServiceAccount => {
            let service_account_json =
                read_json_value("GOOGLE_SERVICE_ACCOUNT_JSON", "GOOGLE_SERVICE_ACCOUNT_PATH")?;
            Ok(GeminiBuilder::new_without_api_key()
                .with_service_account_json(&service_account_json)?
                .with_google_cloud(&project_id()?, &location)
                .with_model(model)
                .build()?)
        }
        SdkMode::VertexWif => {
            let wif_json = read_json_value("GOOGLE_WIF_JSON", "GOOGLE_WIF_PATH")?;
            let project_id = project_id()?;
            Ok(GeminiBuilder::new_without_api_key()
                .with_google_cloud_wif_json(&wif_json)?
                .with_google_cloud(&project_id, &location)
                .with_model(model)
                .build()?)
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();

    let mode = SdkMode::from_env();
    let model = env::var("GEMINI_MODEL").unwrap_or_else(|_| "models/gemini-2.5-flash".to_string());
    let prompt = env::var("ROADMAP_PROMPT")
        .unwrap_or_else(|_| "List two concrete advantages of v1 API stability.".to_string());
    let run_embed =
        env::var("ROADMAP_RUN_EMBED").unwrap_or_else(|_| "1".to_string()).to_lowercase();
    let run_embed = matches!(run_embed.as_str(), "1" | "true" | "yes");

    let client = build_generate_client(mode, model.clone())?;
    let response = client.generate_content().with_user_message(&prompt).execute().await?;

    println!("Mode: {:?}\nModel: {}", mode, model);
    println!("\nGenerate response:\n{}\n", response.text());

    if run_embed {
        let embedding_client = build_embedding_client(mode)?;
        match embedding_client
            .embed_content()
            .with_text("roadmap feature validation embedding probe")
            .with_task_type(TaskType::RetrievalDocument)
            .execute()
            .await
        {
            Ok(embedding) => {
                println!("Embedding succeeded. Length: {}", embedding.embedding.values.len());
            }
            Err(err) => {
                println!("Embedding call reached provider but returned error: {}", err);
            }
        }
    }

    Ok(())
}
