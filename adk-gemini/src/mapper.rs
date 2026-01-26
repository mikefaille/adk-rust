use google_cloud_aiplatform_v1::model as vertex;
use crate::{
    generation::model::{GenerateContentRequest, GenerationResponse},
    models::{Content, Part, Role, Blob},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

// --- From adk-gemini to Vertex AI ---

impl From<GenerateContentRequest> for vertex::GenerateContentRequest {
    fn from(req: GenerateContentRequest) -> Self {
        let mut builder = vertex::GenerateContentRequest::new()
            .set_contents(req.contents.into_iter().map(Into::<vertex::Content>::into).collect::<Vec<_>>())
            .set_tools(req.tools.map(|tools| {
                tools.into_iter().map(Into::<vertex::Tool>::into).collect::<Vec<_>>()
            }).unwrap_or_default());

        if let Some(config) = req.tool_config {
            builder = builder.set_tool_config(Into::<vertex::ToolConfig>::into(config));
        }

        builder
    }
}

impl From<Content> for vertex::Content {
    fn from(content: Content) -> Self {
        vertex::Content::new()
            .set_role(content.role.map(|r| match r {
                Role::User => "user".to_string(),
                Role::Model => "model".to_string(),
            }).unwrap_or_default())
            .set_parts(content.parts.map(|parts| {
                parts.into_iter().map(Into::<vertex::Part>::into).collect::<Vec<_>>()
            }).unwrap_or_default())
    }
}

impl From<Part> for vertex::Part {
    fn from(part: Part) -> Self {
        let mut p = vertex::Part::new();
        match part {
            Part::Text { text, .. } => {
                p = p.set_text(text);
            }
            Part::InlineData { inline_data } => {
                p = p.set_inline_data(vertex::Blob::new()
                    .set_mime_type(inline_data.mime_type)
                    .set_data(bytes::Bytes::from(BASE64.decode(inline_data.data).unwrap_or_default()))
                );
            }
            Part::FunctionCall { function_call, .. } => {
                p = p.set_function_call(vertex::FunctionCall::new()
                    .set_name(function_call.name)
                    .set_args(serde_json::from_value::<google_cloud_wkt::Struct>(function_call.args).unwrap_or_default())
                );
            }
            Part::FunctionResponse { function_response } => {
                p = p.set_function_response(vertex::FunctionResponse::new()
                    .set_name(function_response.name)
                    .set_response(serde_json::from_value::<google_cloud_wkt::Struct>(function_response.response.unwrap_or_default()).unwrap_or_default())
                );
            }
        }
        p
    }
}

impl From<crate::tools::Tool> for vertex::Tool {
    fn from(tool: crate::tools::Tool) -> Self {
        vertex::Tool::new()
            .set_function_declarations(if let crate::tools::Tool::Function { function_declarations } = tool {
                function_declarations.into_iter().map(Into::<vertex::FunctionDeclaration>::into).collect::<Vec<_>>()
            } else {
                Vec::new()
            })
    }
}

impl From<crate::tools::FunctionDeclaration> for vertex::FunctionDeclaration {
    fn from(decl: crate::tools::FunctionDeclaration) -> Self {
        vertex::FunctionDeclaration::new()
            .set_name(decl.name)
            .set_description(decl.description)
            .set_parameters(decl.parameters.map(|p| serde_json::from_value::<vertex::Schema>(serde_json::to_value(p).unwrap()).unwrap()).unwrap_or_default())
    }
}

impl From<crate::tools::ToolConfig> for vertex::ToolConfig {
    fn from(config: crate::tools::ToolConfig) -> Self {
        vertex::ToolConfig::new()
            .set_function_calling_config(config.function_calling_config.map(|fcc| vertex::FunctionCallingConfig::new()
                .set_mode(match fcc.mode {
                    crate::tools::FunctionCallingMode::Auto => vertex::function_calling_config::Mode::Auto,
                    crate::tools::FunctionCallingMode::Any => vertex::function_calling_config::Mode::Any,
                    crate::tools::FunctionCallingMode::None => vertex::function_calling_config::Mode::None,
                })).unwrap_or_default())
    }
}


// --- From Vertex AI to adk-gemini ---

impl From<vertex::GenerateContentResponse> for GenerationResponse {
    fn from(resp: vertex::GenerateContentResponse) -> Self {
        GenerationResponse {
            candidates: resp.candidates.into_iter().map(Into::into).collect(),
            prompt_feedback: resp.prompt_feedback.map(|pf| crate::generation::model::PromptFeedback {
                safety_ratings: pf.safety_ratings.into_iter().map(Into::into).collect(),
                block_reason: Some(crate::generation::model::BlockReason::BlockReasonUnspecified), // Simplified
            }),
            usage_metadata: resp.usage_metadata.map(|um| crate::generation::model::UsageMetadata {
                prompt_token_count: Some(um.prompt_token_count),
                candidates_token_count: Some(um.candidates_token_count),
                total_token_count: Some(um.total_token_count),
                thoughts_token_count: None, // Not directly available in basic UsageMetadata
                prompt_tokens_details: None,
                cached_content_token_count: None,
                cache_tokens_details: None,
            }),
            model_version: None, // Not provided in Vertex response?
            response_id: None,
        }
    }
}

impl From<vertex::Candidate> for crate::generation::model::Candidate {
    fn from(candidate: vertex::Candidate) -> Self {
        crate::generation::model::Candidate {
            content: candidate.content.map(Into::into).unwrap_or_default(),
            finish_reason: Some(match candidate.finish_reason {
                vertex::candidate::FinishReason::Unspecified => crate::generation::model::FinishReason::FinishReasonUnspecified,
                vertex::candidate::FinishReason::Stop => crate::generation::model::FinishReason::Stop,
                vertex::candidate::FinishReason::MaxTokens => crate::generation::model::FinishReason::MaxTokens,
                vertex::candidate::FinishReason::Safety => crate::generation::model::FinishReason::Safety,
                vertex::candidate::FinishReason::Recitation => crate::generation::model::FinishReason::Recitation,
                vertex::candidate::FinishReason::Other => crate::generation::model::FinishReason::Other,
                _ => crate::generation::model::FinishReason::Other,
            }),
            safety_ratings: Some(candidate.safety_ratings.into_iter().map(Into::into).collect()),
            citation_metadata: candidate.citation_metadata.map(|cm| crate::generation::model::CitationMetadata {
                citation_sources: cm.citations.into_iter().map(|c| crate::generation::model::CitationSource {
                    uri: Some(c.uri),
                    title: Some(c.title),
                    start_index: Some(c.start_index),
                    end_index: Some(c.end_index),
                    license: Some(c.license),
                    publication_date: None,
                }).collect(),
            }),
            grounding_metadata: None,
            index: Some(candidate.index),
        }
    }
}

impl From<vertex::Content> for Content {
    fn from(content: vertex::Content) -> Self {
        Content {
            role: Some(match content.role.as_str() {
                "user" => Role::User,
                "model" => Role::Model,
                _ => Role::Model, // Default
            }),
            parts: Some(content.parts.into_iter().map(Into::into).collect()),
        }
    }
}

impl From<vertex::Part> for Part {
    fn from(part: vertex::Part) -> Self {
        match part.data {
            Some(vertex::part::Data::Text(text)) => Part::Text {
                text,
                thought: Some(part.thought),
                thought_signature: None,
            },
            Some(vertex::part::Data::InlineData(blob)) => Part::InlineData {
                inline_data: Blob {
                    mime_type: blob.mime_type,
                    data: BASE64.encode(blob.data),
                }
            },
            Some(vertex::part::Data::FunctionCall(fc)) => Part::FunctionCall {
                function_call: crate::tools::FunctionCall {
                    name: fc.name,
                    args: serde_json::to_value(fc.args).unwrap_or(serde_json::Value::Null),
                    thought_signature: None,
                },
                thought_signature: None,
            },
            Some(vertex::part::Data::FunctionResponse(fr)) => Part::FunctionResponse {
                function_response: crate::tools::FunctionResponse {
                    name: fr.name,
                    response: Some(serde_json::to_value(fr.response).unwrap_or(serde_json::Value::Null)),
                }
            },
            _ => Part::Text { text: String::new(), thought: None, thought_signature: None }, // Fallback
        }
    }
}

impl From<vertex::SafetyRating> for crate::safety::SafetyRating {
    fn from(rating: vertex::SafetyRating) -> Self {
        crate::safety::SafetyRating {
            category: match rating.category {
                vertex::HarmCategory::Unspecified => crate::safety::HarmCategory::Unspecified,
                vertex::HarmCategory::HateSpeech => crate::safety::HarmCategory::HateSpeech,
                vertex::HarmCategory::DangerousContent => crate::safety::HarmCategory::DangerousContent,
                vertex::HarmCategory::Harassment => crate::safety::HarmCategory::Harassment,
                vertex::HarmCategory::SexuallyExplicit => crate::safety::HarmCategory::SexuallyExplicit,
                _ => crate::safety::HarmCategory::Unspecified,
            },
            probability: match rating.probability {
                vertex::safety_rating::HarmProbability::Unspecified => crate::safety::HarmProbability::HarmProbabilityUnspecified,
                vertex::safety_rating::HarmProbability::Negligible => crate::safety::HarmProbability::Negligible,
                vertex::safety_rating::HarmProbability::Low => crate::safety::HarmProbability::Low,
                vertex::safety_rating::HarmProbability::Medium => crate::safety::HarmProbability::Medium,
                vertex::safety_rating::HarmProbability::High => crate::safety::HarmProbability::High,
                _ => crate::safety::HarmProbability::HarmProbabilityUnspecified,
            },
        }
    }
}
