# mistral.rs Integration

## Overview

The `adk-mistralrs` crate provides native [mistral.rs](https://github.com/EricLBuehler/mistral.rs) integration for ADK-Rust, enabling blazingly fast local LLM inference without external dependencies like Ollama.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        ADK Application                          │
├─────────────────────────────────────────────────────────────────┤
│                         adk-agent                               │
│                    (LlmAgent, workflows)                        │
├─────────────────────────────────────────────────────────────────┤
│                         adk-core                                │
│                    (Llm trait interface)                        │
├──────────────────┬──────────────────┬───────────────────────────┤
│    adk-model     │   adk-mistralrs  │      (other crates)       │
│   (crates.io)    │    (git only)    │                           │
├──────────────────┼──────────────────┼───────────────────────────┤
│ Gemini, OpenAI,  │   mistralrs      │                           │
│ Anthropic, etc.  │ (git dependency) │                           │
└──────────────────┴──────────────────┴───────────────────────────┘
```

### Why a Separate Crate?

mistral.rs depends on the `candle` ML framework from HuggingFace, which uses git dependencies. crates.io doesn't allow publishing crates with git dependencies, so:

- `adk-model` remains publishable to crates.io (Gemini, OpenAI, Anthropic, Ollama)
- `adk-mistralrs` is a standalone crate added via git dependency
- Users get the best of both worlds: crates.io convenience + native local inference

## Feature Comparison: mistral.rs vs Ollama

| Feature | adk-mistralrs | adk-model (Ollama) |
|---------|---------------|-------------------|
| **Daemon Required** | No | Yes (ollama serve) |
| **ISQ Quantization** | Yes (on-the-fly) | No |
| **UQFF Pre-quantized** | Yes | No |
| **PagedAttention** | Yes | Limited |
| **FlashAttention** | Yes (CUDA) | Limited |
| **Multi-GPU Splitting** | Yes (layer-based) | Limited |
| **LoRA Adapters** | Yes | No |
| **X-LoRA Dynamic Mixing** | Yes | No |
| **Adapter Hot-Swap** | Yes | No |
| **Vision Models** | Yes (LLaVa, Qwen-VL, etc.) | Yes |
| **Speech Models** | Yes (Dia 1.6b) | No |
| **Diffusion Models** | Yes (FLUX.1) | No |
| **Embedding Models** | Yes | Yes |
| **MCP Integration** | Yes (native) | Via adk-tool |
| **MatFormer (Gemma 3n)** | Yes | No |
| **Custom Chat Templates** | Yes | Limited |
| **Topology Files** | Yes | No |
| **Memory Efficiency** | Excellent | Good |
| **Startup Time** | Slower (model loading) | Faster (daemon) |
| **Model Management** | Manual | Automatic (ollama pull) |

### When to Use Each

**Use adk-mistralrs when:**
- You need ISQ quantization for memory efficiency
- You want LoRA/X-LoRA adapter support
- You need multi-GPU model splitting
- You want speech or diffusion model support
- You prefer no external daemon dependencies
- You need fine-grained control over model loading

**Use Ollama when:**
- You want simple model management (ollama pull)
- You need quick startup times
- You're running multiple applications sharing models
- You prefer a daemon-based architecture
- You want automatic model updates

## Crate Structure

```
adk-mistralrs/
├── Cargo.toml
├── README.md
├── src/
│   ├── lib.rs           # Public exports, crate documentation
│   ├── config.rs        # MistralRsConfig, enums, builder
│   ├── client.rs        # MistralRsModel (basic Llm implementation)
│   ├── adapter.rs       # MistralRsAdapterModel (LoRA/X-LoRA)
│   ├── vision.rs        # MistralRsVisionModel
│   ├── speech.rs        # MistralRsSpeechModel
│   ├── diffusion.rs     # MistralRsDiffusionModel
│   ├── embedding.rs     # MistralRsEmbeddingModel
│   ├── multimodel.rs    # MistralRsMultiModel
│   ├── mcp.rs           # MCP client integration
│   ├── realtime.rs      # RealtimeModel trait implementation
│   ├── convert.rs       # ADK ↔ mistral.rs type conversions
│   └── error.rs         # MistralRsError enum
└── tests/
    ├── config_property_tests.rs
    ├── adapter_property_tests.rs
    ├── tool_conversion_tests.rs
    ├── chat_template_tests.rs
    ├── llm_trait_tests.rs
    └── ...
```

## Key Components

### MistralRsModel

Basic text model implementing the `Llm` trait:

```rust
pub struct MistralRsModel {
    model: Arc<mistralrs::Model>,
    name: String,
    config: MistralRsConfig,
}

impl Llm for MistralRsModel {
    fn name(&self) -> &str;
    async fn generate_content(&self, request: LlmRequest, stream: bool) -> Result<LlmResponseStream>;
}
```

### MistralRsAdapterModel

Model with LoRA/X-LoRA adapter support:

```rust
pub struct MistralRsAdapterModel {
    model: Arc<mistralrs::Model>,
    name: String,
    config: MistralRsConfig,
    active_adapter: RwLock<Option<String>>,
    available_adapters: HashSet<String>,
}

impl MistralRsAdapterModel {
    pub fn available_adapters(&self) -> Vec<String>;
    pub async fn swap_adapter(&self, name: &str) -> Result<()>;
    pub async fn active_adapter(&self) -> Option<String>;
}
```

### MistralRsConfig

Comprehensive configuration with builder pattern:

```rust
pub struct MistralRsConfig {
    pub model_source: ModelSource,      // HuggingFace, Local, GGUF, UQFF
    pub architecture: ModelArchitecture, // Plain, Vision, Diffusion, Speech, Embedding
    pub dtype: DataType,                // F32, F16, BF16, Auto
    pub device: DeviceConfig,           // CPU, CUDA, Metal, Auto
    pub isq: Option<IsqConfig>,         // Quantization settings
    pub adapter: Option<AdapterConfig>, // LoRA/X-LoRA config
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub top_k: Option<i32>,
    pub max_tokens: Option<i32>,
    pub num_ctx: Option<usize>,
    pub paged_attention: bool,
    pub topology_path: Option<PathBuf>,
    pub chat_template: Option<String>,
    pub tokenizer_path: Option<PathBuf>,
    pub matformer: Option<MatFormerConfig>,
    pub mcp_client: Option<McpClientConfig>,
}
```

## Supported Models

### Text Models
- Mistral (7B, 8x7B MoE)
- Llama 2/3 (7B, 13B, 70B)
- Phi-3/3.5 (mini, small, medium)
- Qwen 2/2.5
- Gemma 2/3
- DeepSeek
- CodeLlama
- Mixtral

### Vision Models
- LLaVa (1.5, 1.6, NeXT)
- Qwen-VL
- Gemma 3 (with vision)
- Phi-3 Vision
- Idefics 2

### Speech Models
- Dia 1.6b (text-to-speech)

### Diffusion Models
- FLUX.1 (image generation)

### Embedding Models
- EmbeddingGemma
- Qwen3 Embedding
- BGE (various sizes)
- E5 (various sizes)

## Feature Flags

| Feature | Description |
|---------|-------------|
| `metal` | Apple Metal acceleration (macOS) |
| `cuda` | NVIDIA CUDA acceleration |
| `flash-attn` | Flash Attention (requires CUDA) |
| `cudnn` | cuDNN acceleration |
| `mkl` | Intel MKL acceleration |
| `accelerate` | Apple Accelerate framework |
| `reqwest` | URL-based image loading |

## Performance Considerations

### Memory Optimization

1. **ISQ Quantization**: Reduce memory by 50-88% depending on level
2. **PagedAttention**: Efficient memory for long contexts
3. **Multi-GPU Splitting**: Distribute large models across GPUs

### Startup Time

Model loading is slower than Ollama because:
- Models are loaded directly into memory
- ISQ quantization happens at load time
- No daemon caching

Mitigations:
- Use UQFF pre-quantized models for faster startup
- Keep model instances alive for reuse
- Use multi-model server for shared loading

### Inference Speed

Generally faster than Ollama due to:
- No HTTP overhead
- Direct memory access
- Optimized attention implementations

## Testing Strategy

### Property-Based Tests

15 correctness properties validated with proptest:

1. Llm Trait Implementation Completeness
2. Tool Declaration Conversion Roundtrip
3. Generation Config Propagation
4. Config Variant Completeness
5. Error Message Quality
6. Image Content Handling
7. Embedding Output Format
8. Diffusion Config Completeness
9. Voice Config Completeness
10. Context Length Configuration
11. Adapter Loading and Swapping
12. Multi-Model Routing
13. MCP Tool Discovery
14. Audio Input Handling
15. Chat Template Application

### Integration Tests

Require actual model files (marked `#[ignore]`):
- Basic generation
- Tool calling
- Vision model
- ISQ quantization
- LoRA adapters

## Future Enhancements

### Planned
- Speculative decoding support
- AnyMoE integration
- Distributed inference across machines
- Model caching layer
- Automatic model selection based on hardware

### Under Consideration
- WebGPU backend for browser deployment
- ONNX model support
- TensorRT optimization
- Quantization-aware training integration

## References

- [mistral.rs GitHub](https://github.com/EricLBuehler/mistral.rs)
- [mistral.rs Documentation](https://ericlbuehler.github.io/mistral.rs/)
- [Candle ML Framework](https://github.com/huggingface/candle)
- [ADK-Rust Documentation](https://adk-rust.com)
