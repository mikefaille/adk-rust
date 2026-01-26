use adk_core::{
    Content, FinishReason, Llm, LlmRequest, LlmResponse, LlmResponseStream, Part, Result,
    UsageMetadata,
};
use adk_gemini::{GeminiProvider, GenerateContentRequest, GenerationConfig, part};
use async_trait::async_trait;
use std::collections::HashMap;

pub struct GeminiModel {
    client: GeminiProvider,
    model_name: String,
}

impl GeminiModel {
    pub async fn new(project_id: impl Into<String>, location: impl Into<String>, model: impl Into<String>) -> Result<Self> {
        let client = GeminiProvider::new(project_id, location)
            .await
            .map_err(|e| adk_core::AdkError::Model(e.to_string()))?;

        Ok(Self { client, model_name: model.into() })
    }

    fn convert_response(resp: &adk_gemini::GenerateContentResponse) -> Result<LlmResponse> {
        let mut converted_parts: Vec<Part> = Vec::new();

        // Convert content parts
        if let Some(candidate) = resp.candidates.first() {
            if let Some(content) = &candidate.content {
                for p in &content.parts {
                    if let Some(data) = &p.data {
                        match data {
                            part::Data::Text(text) => {
                                converted_parts.push(Part::Text { text: text.clone() });
                            }
                            part::Data::FunctionCall(func_call) => {
                                // Convert prost Struct to serde Value
                                let args = if let Some(_s) = &func_call.args {
                                    // TODO: Implement proper conversion from prost_types::Struct to serde_json::Value
                                    serde_json::Value::Object(serde_json::Map::new())
                                } else {
                                    serde_json::Value::Null
                                };

                                converted_parts.push(Part::FunctionCall {
                                    name: func_call.name.clone(),
                                    args,
                                    id: None,
                                });
                            }
                            // part::Data::FunctionResponse? usually in request, not response from model
                            _ => {}
                        }
                    }
                }
            }
        }

        let content = if converted_parts.is_empty() {
            None
        } else {
            Some(Content { role: "model".to_string(), parts: converted_parts })
        };

        let usage_metadata = resp.usage_metadata.as_ref().map(|u| UsageMetadata {
            prompt_token_count: u.prompt_token_count,
            candidates_token_count: u.candidates_token_count,
            total_token_count: u.total_token_count,
        });

        // Finish reason mapping
        let finish_reason = resp.candidates.first().map(|c| match c.finish_reason {
            1 => FinishReason::Stop, // STOP
            2 => FinishReason::MaxTokens, // MAX_TOKENS
            3 => FinishReason::Safety, // SAFETY
            _ => FinishReason::Other,
        });

        Ok(LlmResponse {
            content,
            usage_metadata,
            finish_reason,
            partial: false,
            turn_complete: true,
            interrupted: false,
            error_code: None,
            error_message: None,
        })
    }
}

#[async_trait]
impl Llm for GeminiModel {
    fn name(&self) -> &str {
        &self.model_name
    }

    #[adk_telemetry::instrument(
        name = "call_llm",
        skip(self, req),
        fields(
            model.name = %self.model_name,
            stream = %stream,
            request.contents_count = %req.contents.len(),
            request.tools_count = %req.tools.len()
        )
    )]
    async fn generate_content(&self, req: LlmRequest, stream: bool) -> Result<LlmResponseStream> {
        adk_telemetry::info!("Generating content");

        let mut contents = Vec::new();

        for content in req.contents {
            let mut parts = Vec::new();
            for part in content.parts {
                let data = match part {
                    Part::Text { text } => Some(part::Data::Text(text)),
                    Part::InlineData { data, mime_type } => {
                        Some(part::Data::InlineData(adk_gemini::Blob {
                            mime_type,
                            data: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, data).into(),
                        }))
                    },
                    Part::FunctionCall { name, args: _, .. } => {
                        Some(part::Data::FunctionCall(adk_gemini::FunctionCall {
                            name,
                            args: None, // TODO: Convert serde Value to prost Struct
                            ..Default::default()
                        }))
                    },
                    Part::FunctionResponse { function_response, .. } => {
                         Some(part::Data::FunctionResponse(adk_gemini::FunctionResponse {
                            name: function_response.name,
                            response: None, // TODO: Convert serde Value to prost Struct
                            ..Default::default()
                        }))
                    },
                    _ => None,
                };

                if let Some(d) = data {
                    parts.push(adk_gemini::Part {
                        data: Some(d),
                        ..Default::default()
                    });
                }
            }

            contents.push(adk_gemini::Content {
                role: content.role,
                parts,
            });
        }

        // Config mapping
        let generation_config = if let Some(config) = req.config {
            Some(GenerationConfig {
                temperature: Some(config.temperature.unwrap_or(0.0)),
                top_p: Some(config.top_p.unwrap_or(0.0)),
                top_k: Some(config.top_k.unwrap_or(0) as f32),
                candidate_count: Some(1),
                max_output_tokens: Some(config.max_output_tokens.unwrap_or(0)),
                stop_sequences: vec![],
                response_mime_type: "".to_string(),
                presence_penalty: Some(0.0),
                frequency_penalty: Some(0.0),
                ..Default::default()
            })
        } else {
            None
        };

        let request = GenerateContentRequest {
            model: self.model_name.clone(),
            contents,
            tools: vec![], // TODO: Map tools
            tool_config: None,
            safety_settings: vec![],
            generation_config,
            system_instruction: None,
            cached_content: "".to_string(),
            labels: HashMap::new(),
            ..Default::default()
        };

        if stream {
            let stream = self.client.stream_generate_content(request).await.map_err(|e| adk_core::AdkError::Model(e.to_string()))?;

            let mapped_stream = async_stream::stream! {
                use futures::StreamExt;
                let mut stream = stream; // tonic stream implements Stream
                while let Some(result) = stream.next().await {
                    match result {
                        Ok(resp) => {
                            match Self::convert_response(&resp) {
                                Ok(llm_resp) => yield Ok(llm_resp),
                                Err(e) => yield Err(e),
                            }
                        }
                        Err(e) => yield Err(adk_core::AdkError::Model(e.to_string())),
                    }
                }
            };

            Ok(Box::pin(mapped_stream))
        } else {
            let response = self.client.generate_content(request).await.map_err(|e| adk_core::AdkError::Model(e.to_string()))?;
            let llm_response = Self::convert_response(&response)?;

            let stream = async_stream::stream! {
                yield Ok(llm_response);
            };

            Ok(Box::pin(stream))
        }
    }
}
