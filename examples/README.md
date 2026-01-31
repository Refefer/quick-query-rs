# Quick-Query Configuration Examples

This folder contains example configurations for various setups.

## Quick Start

1. Copy one of the example configs to your config directory:
   ```bash
   mkdir -p ~/.config/qq
   cp examples/config.basic.toml ~/.config/qq/config.toml
   ```

2. Edit the config and add your API key:
   ```bash
   $EDITOR ~/.config/qq/config.toml
   ```

3. Run qq:
   ```bash
   qq -p "Hello, world!"
   ```

## Example Configs

| File | Description |
|------|-------------|
| `config.basic.toml` | Minimal setup with just OpenAI |
| `config.full.toml` | All available options documented |
| `config.profiles.toml` | Profiles with prompts and parameters |
| `config.openai-compatible.toml` | Use OpenAI-compatible APIs (Together, Groq, etc.) |
| `config.local-llm.toml` | Local LLM setups (Ollama, LM Studio, vLLM, etc.) |
| `config.multi-provider.toml` | Multiple providers configured together |

## Configuration Reference

### Global Settings

```toml
default_provider = "openai"    # Provider to use when --provider not specified
default_model = "gpt-4o"       # Model to use when --model not specified
temperature = 0.7              # Default temperature (0.0-2.0)
max_tokens = 4096              # Default max tokens
```

### Provider Settings

Each provider is configured under `[providers.<name>]`:

```toml
[providers.openai]
api_key = "sk-..."             # API key (can also use env var)
base_url = "https://..."       # API base URL (optional)
default_model = "gpt-4o"       # Default model for this provider

# Extra parameters passed to the API
[providers.openai.parameters]
chat_template_kwargs = { reasoning_effort = "medium" }
```

### Custom Parameters

Some models support custom parameters. These are passed directly to the API:

```toml
[providers.openai.parameters]
chat_template_kwargs = { reasoning_effort = "high" }
```

### Profiles

Profiles bundle provider, prompt, model, and parameters together:

```toml
default_profile = "coding"

[profiles.coding]
provider = "openai"          # Which provider to use
prompt = "coding-agent"      # Named prompt (from [prompts.X])
model = "gpt-4o"             # Optional model override

[profiles.coding.parameters]
chat_template_kwargs = { reasoning_effort = "high" }

[prompts.coding-agent]
prompt = "You are an expert programmer..."
```

Use with: `qq -P coding -p "Write a function"`

### Named Prompts

Store reusable system prompts:

```toml
[prompts.coding]
prompt = """
You are an expert programmer. Help users write clean, efficient code.
- Ask clarifying questions before writing code
- Show diffs for file changes
"""

[prompts.minimal]
prompt = "Be concise. Answer in as few words as possible."
```

Reference in profiles with `prompt = "coding"` or use inline prompts.

## Environment Variables

Instead of putting API keys in the config file, you can use environment variables:

```bash
export OPENAI_API_KEY="sk-..."
export ANTHROPIC_API_KEY="sk-ant-..."
```

## CLI Options

CLI options override config file settings:

```bash
# Override model
qq -m gpt-4-turbo -p "Hello"

# Override base URL (useful for quick testing)
qq --base-url http://localhost:11434/v1 -p "Hello"

# Override provider
qq --provider anthropic -p "Hello"

# Set temperature
qq -t 0.9 -p "Be creative"

# Set max tokens
qq --max-tokens 100 -p "Be brief"

# Add system prompt
qq -s "You are a pirate" -p "Tell me about the sea"

# Disable streaming
qq --no-stream -p "Hello"

# Enable debug output
qq -d -p "Hello"
```

## Common Setups

### Using with Ollama

1. Install Ollama:
   ```bash
   curl -fsSL https://ollama.ai/install.sh | sh
   ```

2. Pull a model:
   ```bash
   ollama pull llama3.1
   ```

3. Configure qq:
   ```toml
   [providers.openai]
   api_key = "ollama"
   base_url = "http://localhost:11434/v1"
   default_model = "llama3.1"
   ```

4. Run:
   ```bash
   qq -p "Hello from local LLM!"
   ```

### Using with OpenRouter

[OpenRouter](https://openrouter.ai) gives you access to many models through one API:

```toml
[providers.openai]
api_key = "your-openrouter-key"
base_url = "https://openrouter.ai/api/v1"
default_model = "anthropic/claude-3.5-sonnet"
```

### Using with Azure OpenAI

```toml
[providers.openai]
api_key = "your-azure-key"
base_url = "https://your-resource.openai.azure.com/openai/deployments/your-deployment"
default_model = "gpt-4"
```

Note: Azure may require additional headers. Full Azure support is planned for a future release.
