# LLM Agent Test Examples

This project demonstrates various LLM agent configurations and features from the ADK-Rust framework.

## Prerequisites

Set your Google API key:
```bash
export GOOGLE_API_KEY="your-api-key-here"
```

Or create a `.env` file with:
```
GOOGLE_API_KEY="your-api-key-here"
```

## Available Examples

| Binary | Feature | Command | Status |
|--------|---------|---------|--------|
| `basic_agent` | Quick Start | `cargo run --bin basic_agent` | ✅ Working |
| `shaped_behavior` | Instruction personalities | `AGENT_TYPE=formal cargo run --bin shaped_behavior` | ✅ Working |
| `instruction_templating` | `{var}` syntax with session state | `cargo run --bin instruction_templating` | ✅ Working |
| `multi_tools` | Weather + calculator | `cargo run --bin multi_tools` | ✅ Working |
| `structured_output` | JSON schema output | `cargo run --bin structured_output` | ✅ Working |
| `complete_example` | Full production agent | `cargo run --bin complete_example` | ✅ Working |

All binaries compile successfully! ✅

## Shaped Behavior Personalities

For `shaped_behavior`, you can try different agent personalities:

```bash
AGENT_TYPE=formal cargo run --bin shaped_behavior
AGENT_TYPE=tutor cargo run --bin shaped_behavior
AGENT_TYPE=storyteller cargo run --bin shaped_behavior
```

Each personality demonstrates how instructions shape agent behavior and response style.

## Running Examples

**Important**: Run all commands from the `llm_agent_test` directory:
```bash
cd doc-test/llm_agent_test
```

1. **Basic Agent**: Simple LLM agent setup
2. **Shaped Behavior**: Shows how instructions create different personalities
3. **Instruction Templating**: Template variables (`{user_name}`, `{user_role}`) replaced with session state values
4. **Multi Tools**: Agent with multiple tools (weather, calculator)
5. **Structured Output**: JSON schema-validated responses
6. **Complete Example**: Production-ready agent with all features

## Example Outputs

**Basic Agent**: Simple Q&A responses  
**Shaped Behavior**: Different personalities for the same question  
**Instruction Templating**: Personalized responses using template variables  
**Multi Tools**: Uses calculator and weather tools automatically  
**Structured Output**: Returns structured JSON data  
**Complete Example**: Full-featured agent with multiple capabilities  

Try asking each agent the same question to see how their personalities and capabilities differ!
