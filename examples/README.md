# ADK Rust Examples

This directory contains 50+ example applications demonstrating how to use the ADK Rust framework.

## Structure

Each example is in its own directory with a `main.rs` file:

```
examples/
├── quickstart/              # Simple weather agent
├── function_tool/           # Custom function tool
├── multiple_tools/          # Agent composition
├── server/                  # REST API server
├── a2a/                     # A2A protocol
├── web/                     # Multi-agent server
├── sequential/              # Sequential workflow
├── sequential_code/         # Code generation workflow
├── parallel/                # Parallel workflow
├── loop_workflow/           # Iterative loop
├── load_artifacts/          # Artifact loading
├── mcp/                     # MCP integration
├── research_paper/          # Full-stack research paper generator
│
├── openai_*/                # OpenAI integration examples
├── anthropic_*/             # Anthropic Claude examples
├── deepseek_*/              # DeepSeek integration examples
│   ├── deepseek_basic/      # Basic chat example
│   ├── deepseek_reasoner/   # Thinking mode with reasoning
│   ├── deepseek_tools/      # Function calling
│   ├── deepseek_thinking_tools/ # Reasoning + tools
│   ├── deepseek_caching/    # Context caching demo
│   ├── deepseek_sequential/ # Multi-agent pipeline
│   ├── deepseek_supervisor/ # Supervisor pattern
│   └── deepseek_structured/ # Structured JSON output
│
├── realtime_*/              # Realtime voice agent examples
│
├── graph_*/                 # Graph workflow examples
│   ├── graph_workflow/      # Basic graph workflow
│   ├── graph_react/         # ReAct pattern with tool loop
│   ├── graph_supervisor/    # Multi-agent supervisor
│   ├── graph_hitl/          # Human-in-the-loop
│   ├── graph_checkpoint/    # State persistence
│   ├── graph_conditional/   # Conditional routing
│   └── graph_llm/           # LLM-powered graph nodes
│
├── browser_*/               # Browser automation examples
│   ├── browser_basic/       # Basic session and tools
│   ├── browser_agent/       # AI agent with browser
│   ├── browser_interactive/ # Full 46-tool example
│   ├── browser_openai/      # OpenAI browser agent
│   └── browser_test/        # Integration tests
│
└── eval_*/                  # Agent evaluation examples
    ├── eval_basic/          # Basic evaluation setup
    ├── eval_trajectory/     # Tool trajectory validation
    ├── eval_semantic/       # LLM-judged matching
    ├── eval_rubric/         # Rubric-based scoring
    ├── eval_similarity/     # Response similarity
    └── eval_report/         # Report generation

├── mistralrs_*/             # mistral.rs local inference examples
│   ├── mistralrs_basic/     # Basic text generation
│   ├── mistralrs_tools/     # Function calling
│   ├── mistralrs_vision/    # Image understanding
│   ├── mistralrs_isq/       # In-situ quantization
│   ├── mistralrs_lora/      # LoRA adapter usage
│   ├── mistralrs_multimodel/# Multi-model serving
│   ├── mistralrs_mcp/       # MCP client integration
│   ├── mistralrs_speech/    # Text-to-speech synthesis
│   └── mistralrs_diffusion/ # Image generation with FLUX
```

## Prerequisites

Set your API keys as needed:
```bash
# Google Gemini
export GOOGLE_API_KEY="your-api-key-here"
# or
export GEMINI_API_KEY="your-api-key-here"

# OpenAI
export OPENAI_API_KEY="your-api-key-here"

# Anthropic
export ANTHROPIC_API_KEY="your-api-key-here"

# DeepSeek
export DEEPSEEK_API_KEY="your-api-key-here"
```

## Running Examples

All examples can be run with:
```bash
cargo run --example <example_name>
```

## Available Examples

### Basic Examples

#### quickstart
A simple weather and time agent using Google Search:
```bash
cargo run --example quickstart
```
Demonstrates: Creating a Gemini model, building an LLM agent with tools, running an interactive console session.

#### function_tool
Calculator agent with custom function tool:
```bash
cargo run --example function_tool
```
Demonstrates: Creating custom function tools, arithmetic operations.

#### multiple_tools
Agent orchestrating multiple sub-agents with different tool types:
```bash
cargo run --example multiple_tools
```
Demonstrates: Sub-agent pattern, mixing GoogleSearch and custom tools, agent composition.

### Server Examples

#### server
Starts an HTTP server with REST and A2A endpoints:
```bash
cargo run --example server
# or with custom port
PORT=3000 cargo run --example server
```
Demonstrates: Server mode, agent loader, HTTP endpoints.

#### a2a
Agent-to-Agent protocol demonstration:
```bash
cargo run --example a2a
```
Demonstrates: A2A agent card generation, protocol integration pattern.

#### web
Multi-agent web application with artifact support:
```bash
cargo run --example web
```
Demonstrates: Multiple specialized agents, MultiAgentLoader, REST server with agent selection.

### Workflow Examples

#### sequential
Sequential workflow processing (analyze → expand → summarize):
```bash
cargo run --example sequential
```
Demonstrates: Sequential agent execution, multi-step processing.

#### sequential_code
Code generation workflow (design → implement → review):
```bash
cargo run --example sequential_code
```
Demonstrates: Sequential workflow for code generation, multi-stage refinement.

#### parallel
Parallel workflow with multiple perspectives (technical, business, user):
```bash
cargo run --example parallel
```
Demonstrates: Concurrent agent execution, parallel analysis.

#### loop_workflow
Iterative refinement loop with exit condition:
```bash
cargo run --example loop_workflow
```
Demonstrates: Loop agent, iterative processing, exit_loop tool.

### Tool Examples

#### load_artifacts
Demonstrate artifact loading and management:
```bash
cargo run --example load_artifacts
```
Demonstrates: LoadArtifactsTool, artifact service integration.

#### mcp
Model Context Protocol integration:
```bash
cargo run --example mcp
```
Demonstrates: McpToolset integration pattern.

### Full-Stack Examples

#### research_paper
Complete client-server application for generating research papers:
```bash
cargo run --example research_paper -- serve --port 8080
```
Then open `examples/research_paper/frontend.html` in your browser.

Demonstrates: 
- Full-stack architecture (frontend + backend)
- Custom research and PDF generation tools
- Real-time SSE streaming to web client
- Artifact storage and download
- Session management
- Production-ready integration patterns

See [research_paper/README.md](research_paper/README.md) for detailed documentation.

### Graph Workflow Examples

Graph-based workflows using LangGraph-style orchestration:

#### graph_workflow
Basic linear graph workflow:
```bash
cargo run --example graph_workflow
```
Demonstrates: Graph builder, nodes, edges, state management.

#### graph_react
ReAct pattern with tool loop:
```bash
cargo run --example graph_react
```
Demonstrates: Cyclic graphs, conditional routing, tool execution.

#### graph_supervisor
Multi-agent supervisor pattern:
```bash
cargo run --example graph_supervisor
```
Demonstrates: Supervisor routing, specialist agents, dynamic delegation.

#### graph_hitl
Human-in-the-loop approval workflow:
```bash
cargo run --example graph_hitl
```
Demonstrates: Checkpointing, interrupts, state resumption.

#### graph_checkpoint
State persistence and recovery:
```bash
cargo run --example graph_checkpoint
```
Demonstrates: SQLite checkpointer, state persistence.

### DeepSeek Examples

DeepSeek model integration with unique features:

#### deepseek_basic
Basic DeepSeek chat:
```bash
cargo run --example deepseek_basic --features deepseek
```
Demonstrates: DeepSeek client setup, basic chat completion.

#### deepseek_reasoner
Reasoning model with thinking mode:
```bash
cargo run --example deepseek_reasoner --features deepseek
```
Demonstrates: Chain-of-thought reasoning, `<thinking>` tags in output.

#### deepseek_tools
Function calling with DeepSeek:
```bash
cargo run --example deepseek_tools --features deepseek
```
Demonstrates: Tool definitions, function calling, weather and calculator tools.

#### deepseek_thinking_tools
Combining reasoning with tool use:
```bash
cargo run --example deepseek_thinking_tools --features deepseek
```
Demonstrates: Financial analysis with stock prices, currency conversion, reasoning + tools.

#### deepseek_caching
Context caching demonstration:
```bash
cargo run --example deepseek_caching --features deepseek
```
Demonstrates: KV cache benefits, document Q&A, cost reduction with repeated prefixes.

#### deepseek_sequential
Multi-agent sequential pipeline:
```bash
cargo run --example deepseek_sequential --features deepseek
```
Demonstrates: Researcher -> Analyst -> Writer pipeline, shared model instance.

#### deepseek_supervisor
Multi-agent supervisor pattern:
```bash
cargo run --example deepseek_supervisor --features deepseek
```
Demonstrates: Dynamic routing, specialist agents, graph-based workflow.

#### deepseek_structured
Structured JSON output:
```bash
cargo run --example deepseek_structured --features deepseek
```
Demonstrates: Output schema, JSON responses, product analysis.

### mistral.rs Local Inference Examples

Native local LLM inference using mistral.rs - no external daemon required:

> **Note:** These examples require the `mistralrs` feature and models will be downloaded on first run.

#### mistralrs_basic
Basic text generation with local model:
```bash
cargo run --example mistralrs_basic --features mistralrs
```
Demonstrates: MistralRsModel setup, HuggingFace model loading, basic chat completion.

#### mistralrs_tools
Function calling with local model:
```bash
cargo run --example mistralrs_tools --features mistralrs
```
Demonstrates: Tool definitions, function calling, weather and calculator tools with local inference.

#### mistralrs_vision
Image understanding with vision model:
```bash
cargo run --example mistralrs_vision --features mistralrs
```
Demonstrates: MistralRsVisionModel, image input handling, multimodal inference.

#### mistralrs_isq
In-situ quantization for memory efficiency:
```bash
cargo run --example mistralrs_isq --features mistralrs
```
Demonstrates: ISQ quantization levels (Q4_0, Q8_0, etc.), memory-efficient inference.

#### mistralrs_lora
LoRA adapter usage and hot-swapping:
```bash
cargo run --example mistralrs_lora --features mistralrs
```
Demonstrates: MistralRsAdapterModel, LoRA adapter loading, runtime adapter swapping.

#### mistralrs_multimodel
Multi-model serving with routing:
```bash
cargo run --example mistralrs_multimodel --features mistralrs
```
Demonstrates: MistralRsMultiModel, model routing by name, concurrent model serving.

#### mistralrs_mcp
MCP client integration with local model:
```bash
cargo run --example mistralrs_mcp --features mistralrs
```
Demonstrates: MCP server connection, tool discovery, local model with MCP tools.

#### mistralrs_speech
Text-to-speech synthesis with Dia models:
```bash
cargo run --example mistralrs_speech --features mistralrs
```
Demonstrates: MistralRsSpeechModel, text-to-speech, multi-speaker dialogue, WAV output.

#### mistralrs_diffusion
Image generation with FLUX models:
```bash
cargo run --example mistralrs_diffusion --features mistralrs
```
Demonstrates: MistralRsDiffusionModel, text-to-image, FLUX.1-schnell, configurable image size.

> **Note:** Diffusion models require significant GPU memory (~12-24GB VRAM).

### Browser Automation Examples

Web browser automation using WebDriver:

**Prerequisites**: Start WebDriver server
```bash
docker run -d -p 4444:4444 selenium/standalone-chrome
```

#### browser_basic
Basic browser session and tools:
```bash
cargo run --example browser_basic
```
Demonstrates: BrowserSession, BrowserToolset, 46 browser tools.

#### browser_agent
AI agent with browser tools:
```bash
cargo run --example browser_agent
```
Demonstrates: LlmAgent with browser tools, web research.

#### browser_interactive
Full 46-tool interactive example:
```bash
cargo run --example browser_interactive
```
Demonstrates: All browser tools, navigation, extraction, forms, screenshots.

### Agent Evaluation Examples

Test and validate agent behavior:

#### eval_basic
Basic evaluation setup:
```bash
cargo run --example eval_basic
```
Demonstrates: Evaluator, EvaluationConfig, test files.

#### eval_trajectory
Tool call trajectory validation:
```bash
cargo run --example eval_trajectory
```
Demonstrates: Trajectory matching, tool sequence validation.

#### eval_semantic
LLM-judged semantic matching:
```bash
cargo run --example eval_semantic
```
Demonstrates: LLM judge, semantic similarity scoring.

#### eval_rubric
Rubric-based scoring:
```bash
cargo run --example eval_rubric
```
Demonstrates: Custom rubrics, weighted criteria.

## Example Categories

| Category | Count | Examples |
|----------|-------|----------|
| **Basic** | 3 | quickstart, function_tool, multiple_tools |
| **Servers** | 3 | server, a2a, web |
| **Workflows** | 4 | sequential, sequential_code, parallel, loop_workflow |
| **Tools** | 2 | load_artifacts, mcp |
| **Full-Stack** | 1 | research_paper |
| **OpenAI** | 4+ | openai_basic, openai_tools, openai_multimodal, etc. |
| **Anthropic** | 2 | anthropic_basic, anthropic_tools |
| **DeepSeek** | 8 | deepseek_basic, deepseek_reasoner, deepseek_tools, etc. |
| **mistral.rs** | 9 | mistralrs_basic, mistralrs_tools, mistralrs_vision, mistralrs_speech, mistralrs_diffusion, etc. |
| **Realtime** | 4+ | realtime_basic, realtime_vad, realtime_tools, etc. |
| **Graph** | 9 | graph_workflow, graph_react, graph_supervisor, etc. |
| **Browser** | 5 | browser_basic, browser_agent, browser_interactive, etc. |
| **Evaluation** | 11 | eval_basic, eval_trajectory, eval_semantic, etc. |
| **Total** | **55+** | |

## Parity with Go ADK

| Go Example | Rust Example | Status |
|------------|--------------|--------|
| quickstart | quickstart | ✅ Complete |
| rest | server | ✅ Complete |
| a2a | a2a | ✅ Complete |
| mcp | mcp | ✅ Complete |
| web | web | ✅ Complete |
| tools/multipletools | multiple_tools | ✅ Complete |
| tools/loadartifacts | load_artifacts | ✅ Complete |
| workflowagents/sequential | sequential | ✅ Complete |
| workflowagents/sequentialCode | sequential_code | ✅ Complete |
| workflowagents/parallel | parallel | ✅ Complete |
| workflowagents/loop | loop_workflow | ✅ Complete |

## Beyond Go ADK

ADK-Rust includes additional features not in the Go implementation:

| Feature | Examples |
|---------|----------|
| **OpenAI Integration** | openai_basic, openai_tools, openai_multimodal |
| **Anthropic Integration** | anthropic_basic, anthropic_tools |
| **DeepSeek Integration** | deepseek_basic, deepseek_reasoner, deepseek_tools, deepseek_thinking_tools, deepseek_caching, deepseek_sequential, deepseek_supervisor, deepseek_structured |
| **mistral.rs Local Inference** | mistralrs_basic, mistralrs_tools, mistralrs_vision, mistralrs_isq, mistralrs_lora, mistralrs_multimodel, mistralrs_mcp, mistralrs_speech, mistralrs_diffusion |
| **Realtime Voice** | realtime_basic, realtime_vad, realtime_tools |
| **Graph Workflows** | graph_react, graph_supervisor, graph_hitl |
| **Browser Automation** | browser_agent, browser_interactive |
| **Agent Evaluation** | eval_trajectory, eval_semantic, eval_rubric |

## Example Structure

Each example is a standalone Rust file that:
1. Loads API key from environment
2. Creates Gemini model(s)
3. Builds agent(s) with tools/sub-agents
4. Runs console or server mode

## Tips

- Use Ctrl+C to exit console mode
- Server mode runs on port 8080 by default (override with PORT env var)
- All examples use `gemini-2.5-flash` model
- Console mode includes readline history and editing
- MCP and A2A examples show integration patterns (placeholders)
