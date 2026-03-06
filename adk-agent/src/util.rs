use adk_core::{model, event};

pub fn map_finish_reason(reason: Option<model::FinishReason>) -> Option<event::FinishReason> {
    reason.map(|r| match r {
        model::FinishReason::Stop => event::FinishReason::Stop,
        model::FinishReason::MaxTokens => event::FinishReason::Length,
        model::FinishReason::Safety => event::FinishReason::Safety,
        model::FinishReason::Recitation => event::FinishReason::Other("Recitation".to_string()),
        model::FinishReason::Other => event::FinishReason::Other("Unknown".to_string()),
    })
}

pub fn map_usage_metadata(usage: Option<model::UsageMetadata>) -> Option<event::UsageMetadata> {
    usage.map(|u| event::UsageMetadata {
        prompt_token_count: u.prompt_token_count as u32,
        candidates_tokens: u.candidates_token_count as u32,
        total_tokens: u.total_token_count as u32,
        cache_read_input_token_count: u.cache_read_input_token_count.map(|c| c as u32),
        cache_creation_input_token_count: u.cache_creation_input_token_count.map(|c| c as u32),
    })
}
