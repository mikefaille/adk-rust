//! Live LLM-agent examples for the v0.8.0 performance work.
//!
//! Each case builds and runs a real LLM agent against one adoption-focused
//! prompt. The example validates each response internally and prints only
//! non-sensitive metadata, not model output.

use adk_rust::GenerateContentConfig;
use adk_rust::prelude::*;
use adk_rust::session::{CreateRequest, InMemorySessionService, SessionService};
use futures::StreamExt;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

struct UseCase {
    finding: &'static str,
    agent_name: &'static str,
    user_prompt: &'static str,
    recommendation_focus: &'static str,
    validation_marker: &'static str,
    run_config: RunConfig,
}

#[tokio::main]
async fn main() -> Result<()> {
    let model = real_model_from_env()?;
    let mut cases = vec![
        UseCase {
            finding: "1. current cargo-adk scaffolds",
            agent_name: "scaffold_advisor",
            user_prompt: "Create a small support bot project I can compile quickly.",
            recommendation_focus: "Use cargo-adk's 0.8 templates with minimal features for the first build.",
            validation_marker: "ADK08-01",
            run_config: RunConfig::default(),
        },
        UseCase {
            finding: "2. rustls-only HTTP clients",
            agent_name: "install_doctor",
            user_prompt: "My Linux install is failing around OpenSSL. What should I try?",
            recommendation_focus: "Use the rustls-backed dependency set; native-tls is no longer part of the starter path.",
            validation_marker: "ADK08-02",
            run_config: RunConfig::default(),
        },
        UseCase {
            finding: "3. true minimal starter tier",
            agent_name: "starter_agent",
            user_prompt: "I need a simple appointment reminder agent with the smallest build.",
            recommendation_focus: "Start with the minimal tier: agent, Gemini model, runner, and sessions.",
            validation_marker: "ADK08-03",
            run_config: RunConfig::default(),
        },
        UseCase {
            finding: "4. opt-in CLI providers",
            agent_name: "cli_provider_advisor",
            user_prompt: "I only use Gemini in the CLI. Do I need every provider compiled?",
            recommendation_focus: "Install the CLI with only the provider features the user needs.",
            validation_marker: "ADK08-04",
            run_config: RunConfig::default(),
        },
        UseCase {
            finding: "5. telemetry core without OTLP",
            agent_name: "telemetry_triage",
            user_prompt: "I want logs locally now and can add OTLP later.",
            recommendation_focus: "Use telemetry core for tracing now, then enable telemetry-otlp when exporting to a collector.",
            validation_marker: "ADK08-05",
            run_config: RunConfig::default(),
        },
        UseCase {
            finding: "6. MCP tools are opt-in",
            agent_name: "tooling_advisor",
            user_prompt: "My agent only calls local Rust function tools.",
            recommendation_focus: "Keep MCP disabled until the agent connects to an MCP server.",
            validation_marker: "ADK08-06",
            run_config: RunConfig::default(),
        },
        UseCase {
            finding: "7. Gemini backtraces are debug-only",
            agent_name: "gemini_debug_advisor",
            user_prompt: "How do I keep release builds lean but preserve deep errors in debug builds?",
            recommendation_focus: "Use the default lean Gemini client and enable the backtrace feature only for debugging.",
            validation_marker: "ADK08-07",
            run_config: RunConfig::default(),
        },
        UseCase {
            finding: "8. empty state-delta session fast path",
            agent_name: "session_budgeter",
            user_prompt: "Most of my turns do not change state. Can persistence be cheaper?",
            recommendation_focus: "Empty state deltas now skip state merge work and only append the event.",
            validation_marker: "ADK08-08",
            run_config: RunConfig::default(),
        },
        UseCase {
            finding: "9. bounded history loading",
            agent_name: "history_window_support",
            user_prompt: "Load only the recent turns for a support chat handoff.",
            recommendation_focus: "Set RunConfig.history_max_events to bound startup work while preserving recent context.",
            validation_marker: "ADK08-09",
            run_config: RunConfig { history_max_events: Some(12), ..RunConfig::default() },
        },
        UseCase {
            finding: "10. payload-safe tracing",
            agent_name: "privacy_observer",
            user_prompt: "Trace enough to debug without leaking long customer payloads.",
            recommendation_focus: "Keep record_payloads off and use trace_payload_max_bytes to cap recorded payload fields.",
            validation_marker: "ADK08-10",
            run_config: RunConfig { trace_payload_max_bytes: 256, ..RunConfig::default() },
        },
        UseCase {
            finding: "11. bounded parallel tools",
            agent_name: "operations_dispatcher",
            user_prompt: "Call several read-only inventory tools without flooding the backend.",
            recommendation_focus: "Set RunConfig.max_tool_concurrency to cap parallel tool execution.",
            validation_marker: "ADK08-11",
            run_config: RunConfig { max_tool_concurrency: Some(4), ..RunConfig::default() },
        },
        UseCase {
            finding: "12. cache lifecycle without mutex-held network waits",
            agent_name: "cache_operator",
            user_prompt: "Refresh prompt caches without blocking other sessions.",
            recommendation_focus: "Cache create/delete calls now run outside the cache manager mutex.",
            validation_marker: "ADK08-12",
            run_config: RunConfig::default(),
        },
    ];

    let total_cases = cases.len();
    for (case_index, case) in cases.drain(..).enumerate() {
        run_case(case, model.clone()).await?;
        println!("case {}/{} completed", case_index + 1, total_cases);
    }

    Ok(())
}

async fn run_case(case: UseCase, model: Arc<dyn Llm>) -> Result<()> {
    let agent = Arc::new(
        LlmAgentBuilder::new(case.agent_name)
            .description(format!("Validation agent for {}", case.finding))
            .instruction(format!(
                "Answer with one practical recommendation for the user's adoption task. \
                 Focus on this validated optimization: {}. \
                 Include the marker {} exactly once so the example can validate the live LLM turn. \
                Do not include secrets, API keys, session IDs, or tool payloads.",
                case.recommendation_focus, case.validation_marker
            ))
            .generate_content_config(GenerateContentConfig {
                temperature: Some(0.2),
                max_output_tokens: Some(160),
                ..GenerateContentConfig::default()
            })
            .model(model)
            .build()?,
    );

    let session_service: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
    let session_id = format!("session_{}", case.agent_name);
    session_service
        .create(CreateRequest {
            app_name: "performance_0_8_examples".to_string(),
            user_id: "user".to_string(),
            session_id: Some(session_id.clone()),
            state: HashMap::new(),
        })
        .await?;

    let runner = Runner::builder()
        .app_name("performance_0_8_examples")
        .agent(agent)
        .session_service(session_service)
        .run_config(case.run_config)
        .build()?;

    let mut stream = runner
        .run_str("user", &session_id, Content::new("user").with_text(case.user_prompt))
        .await?;

    let mut text = String::new();
    while let Some(event) = stream.next().await {
        let event = event?;
        if let Some(content) = event.llm_response.content {
            for part in content.parts {
                if let Some(part_text) = part.text() {
                    text.push_str(part_text);
                }
            }
        }
    }

    if !text.contains(case.validation_marker) {
        return Err(AdkError::agent(format!(
            "{} response did not include the validation marker",
            case.finding,
        )));
    }

    Ok(())
}

fn real_model_from_env() -> Result<Arc<dyn Llm>> {
    #[cfg(feature = "openrouter")]
    {
        if let Some(api_key) = env_value("OPENROUTER_API_KEY") {
            let model_name = env_or("OPENROUTER_MODEL", "openai/gpt-4.1-mini");
            let config = OpenRouterConfig::new(api_key, &model_name)
                .with_base_url(env_or("OPENROUTER_BASE_URL", "https://openrouter.ai/api/v1"))
                .with_http_referer(env_or(
                    "OPENROUTER_SITE_URL",
                    "https://github.com/zavora-ai/adk-rust",
                ))
                .with_title(env_or("OPENROUTER_APP_NAME", "ADK-Rust Performance Example"))
                .with_default_api_mode(OpenRouterApiMode::ChatCompletions);
            return Ok(Arc::new(OpenRouterClient::new(config)?));
        }
    }

    #[cfg(feature = "anthropic")]
    {
        if let Some(api_key) = env_value("ANTHROPIC_API_KEY") {
            return Ok(Arc::new(AnthropicClient::from_api_key(api_key)?));
        }
    }

    #[cfg(feature = "openai")]
    {
        if let Some(api_key) = env_value("OPENAI_API_KEY") {
            let config = OpenAIConfig::new(api_key, env_or("OPENAI_MODEL", "gpt-5-mini"));
            return Ok(Arc::new(OpenAIClient::new(config)?));
        }
    }

    #[cfg(feature = "gemini")]
    {
        if let Some(api_key) = env_value("GOOGLE_API_KEY").or_else(|| env_value("GEMINI_API_KEY")) {
            return Ok(Arc::new(GeminiModel::new(
                api_key,
                env_or("GEMINI_MODEL", "gemini-2.5-flash"),
            )?));
        }
    }

    Err(AdkError::config(
        "No real LLM provider detected. Enable a provider feature and set the matching API key.",
    ))
}

fn env_or(key: &str, default: &str) -> String {
    env_value(key).unwrap_or_else(|| default.to_string())
}

fn env_value(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| dotenv_values().get(key).cloned().filter(|value| !value.trim().is_empty()))
}

fn dotenv_values() -> &'static HashMap<String, String> {
    static VALUES: OnceLock<HashMap<String, String>> = OnceLock::new();
    VALUES.get_or_init(|| {
        find_dotenv_path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|contents| {
                contents.lines().filter_map(parse_dotenv_line).collect::<HashMap<_, _>>()
            })
            .unwrap_or_default()
    })
}

fn find_dotenv_path() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let path = dir.join(".env");
        if path.is_file() {
            return Some(path);
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn parse_dotenv_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let trimmed = trimmed.strip_prefix("export ").unwrap_or(trimmed);
    let (key, value) = trimmed.split_once('=')?;
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    Some((key.to_string(), unquote_dotenv_value(value.trim())))
}

fn unquote_dotenv_value(value: &str) -> String {
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        let quote = bytes[0];
        if (quote == b'"' || quote == b'\'') && bytes[value.len() - 1] == quote {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}
