use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

use qq_core::{CompletionRequest, Message, Provider, ToolCall, ToolRegistry};
use qq_providers::openai::OpenAIProvider;

mod chat;
mod config;

use config::{expand_path, Config};

#[derive(Parser)]
#[command(name = "qq")]
#[command(author, version, about = "Quick-query: A fast LLM CLI tool", long_about = None)]
pub struct Cli {
    /// Prompt to send (for quick completion mode)
    #[arg(short, long)]
    pub prompt: Option<String>,

    /// Profile to use (bundles provider, prompt, model, parameters)
    #[arg(short = 'P', long)]
    pub profile: Option<String>,

    /// Model to use (overrides config/profile default)
    #[arg(short, long)]
    pub model: Option<String>,

    /// Provider to use (overrides profile)
    #[arg(long)]
    pub provider: Option<String>,

    /// Base URL for the API (overrides config)
    #[arg(long)]
    pub base_url: Option<String>,

    /// System prompt (overrides profile)
    #[arg(short, long)]
    pub system: Option<String>,

    /// Temperature (0.0-2.0)
    #[arg(short, long)]
    pub temperature: Option<f32>,

    /// Maximum tokens to generate
    #[arg(long)]
    pub max_tokens: Option<u32>,

    /// Disable streaming output
    #[arg(long)]
    pub no_stream: bool,

    /// Enable debug output
    #[arg(short, long)]
    pub debug: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start interactive chat mode
    Chat {
        /// Initial system prompt for the conversation
        #[arg(short, long)]
        system: Option<String>,
    },
    /// List available models for a provider
    Models,
    /// Show current configuration
    Config,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Set up logging
    let filter = if cli.debug {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("warn")
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();

    // Load configuration
    let config = Config::load()?;

    match &cli.command {
        Some(Commands::Chat { system }) => {
            chat_mode(&cli, &config, system.clone()).await
        }
        Some(Commands::Models) => {
            list_models(&cli, &config).await
        }
        Some(Commands::Config) => {
            show_config(&config)
        }
        None => {
            if let Some(prompt) = &cli.prompt {
                completion_mode(&cli, &config, prompt).await
            } else {
                // Default to chat mode if no prompt provided
                chat_mode(&cli, &config, cli.system.clone()).await
            }
        }
    }
}

/// Build tools registry from config
fn build_tools_registry(config: &Config) -> Result<ToolRegistry> {
    // Resolve root directory: config > $PWD
    let root = config.tools.root.as_ref()
        .map(|s| expand_path(s))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // Resolve memory db: config > default
    let memory_db = config.tools.memory_db.as_ref()
        .map(|s| expand_path(s))
        .or_else(|| Config::config_dir().ok().map(|d| d.join("memory.db")));

    let mut registry = ToolRegistry::new();

    // Filesystem tools
    if config.tools.enable_filesystem {
        let fs_config = qq_tools::FileSystemConfig::new(&root).with_write(config.tools.allow_write);
        for tool in qq_tools::create_filesystem_tools(fs_config) {
            registry.register(tool);
        }
    }

    // Memory tools
    if config.tools.enable_memory {
        let store = if let Some(db_path) = &memory_db {
            std::sync::Arc::new(qq_tools::MemoryStore::new(db_path)?)
        } else {
            std::sync::Arc::new(qq_tools::MemoryStore::in_memory()?)
        };
        for tool in qq_tools::create_memory_tools(store) {
            registry.register(tool);
        }
    }

    // Web tools
    if config.tools.enable_web {
        for tool in qq_tools::create_web_tools() {
            registry.register(tool);
        }
    }

    Ok(registry)
}

async fn completion_mode(cli: &Cli, config: &Config, prompt: &str) -> Result<()> {
    // Resolve settings from profile, CLI, and config
    let settings = resolve_settings(cli, config)?;
    let provider = create_provider_from_settings(&settings)?;

    // Set up tools
    let tools_registry = build_tools_registry(config)?;

    let mut messages = Vec::new();

    // Add system prompt (CLI overrides profile)
    if let Some(system) = &settings.system_prompt {
        messages.push(Message::system(system.as_str()));
    }

    messages.push(Message::user(prompt));

    // Agentic loop - keep going while LLM returns tool calls
    let max_iterations = 20;
    for iteration in 0..max_iterations {
        let mut request = CompletionRequest::new(messages.clone());

        // Apply model
        if let Some(model) = &settings.model {
            request = request.with_model(model.as_str());
        }

        if let Some(temp) = cli.temperature {
            request = request.with_temperature(temp);
        }

        if let Some(max_tokens) = cli.max_tokens {
            request = request.with_max_tokens(max_tokens);
        }

        // Add merged parameters
        if !settings.parameters.is_empty() {
            request = request.with_extra(settings.parameters.clone());
        }

        // Add tool definitions
        request = request.with_tools(tools_registry.definitions());

        // Non-streaming mode for tool handling
        let response = provider.complete(request).await?;

        // Check if we have tool calls
        if !response.message.tool_calls.is_empty() {
            // Print assistant message if any
            let content = response.message.content.to_string_lossy();
            if !content.is_empty() {
                println!("{}", content);
            }

            // Add assistant message to history
            messages.push(response.message.clone());

            // Execute each tool call
            let tool_calls = response.message.tool_calls.clone();
            for tool_call in tool_calls {
                if cli.debug {
                    eprintln!("[tool] {}({})", tool_call.name, tool_call.arguments);
                }

                let result = execute_tool_call(&tools_registry, &tool_call).await;

                if cli.debug {
                    let preview = if result.len() > 200 {
                        format!("{}...", &result[..200])
                    } else {
                        result.clone()
                    };
                    eprintln!("[tool result] {}", preview);
                }

                // Add tool result to messages
                messages.push(Message::tool_result(&tool_call.id, result));
            }

            // Continue loop to get next response
            continue;
        }

        // No tool calls - print final response and exit
        println!("{}", response.message.content.to_string_lossy());

        if cli.debug {
            eprintln!(
                "[tokens: {} prompt, {} completion, {} total | iterations: {}]",
                response.usage.prompt_tokens,
                response.usage.completion_tokens,
                response.usage.total_tokens,
                iteration + 1
            );
        }

        return Ok(());
    }

    eprintln!("Warning: Max iterations ({}) reached", max_iterations);
    Ok(())
}

async fn execute_tool_call(registry: &ToolRegistry, tool_call: &ToolCall) -> String {
    let Some(tool) = registry.get(&tool_call.name) else {
        return format!("Error: Unknown tool '{}'", tool_call.name);
    };

    match tool.execute(tool_call.arguments.clone()).await {
        Ok(output) => {
            if output.is_error {
                format!("Error: {}", output.content)
            } else {
                output.content
            }
        }
        Err(e) => format!("Error executing tool: {}", e),
    }
}

async fn chat_mode(cli: &Cli, config: &Config, system: Option<String>) -> Result<()> {
    // Resolve settings from profile, CLI, and config
    let settings = resolve_settings(cli, config)?;
    let provider = create_provider_from_settings(&settings)?;

    // Determine system prompt: explicit arg > CLI > profile
    let system_prompt = system
        .or_else(|| cli.system.clone())
        .or(settings.system_prompt);

    // Set up tools
    let tools_registry = build_tools_registry(config)?;

    chat::run_chat(
        cli,
        config,
        provider,
        system_prompt,
        tools_registry,
        settings.parameters,
        settings.model,
    )
    .await
}

async fn list_models(cli: &Cli, config: &Config) -> Result<()> {
    let provider = create_provider(cli, config)?;
    println!("Available models for {}:", provider.name());
    for model in provider.available_models() {
        println!("  - {}", model);
    }
    Ok(())
}

fn show_config(config: &Config) -> Result<()> {
    println!("Configuration:");
    println!("  Default profile: {}", config.default_profile);

    if !config.profiles.is_empty() {
        println!("\nProfiles:");
        for (name, profile) in &config.profiles {
            println!("  {}:", name);
            if let Some(provider) = &profile.provider {
                println!("    Provider: {}", provider);
            }
            if let Some(prompt) = &profile.prompt {
                // Show truncated prompt name or inline prompt
                let display = if config.prompts.contains_key(prompt) {
                    format!("@{}", prompt)
                } else if prompt.len() > 50 {
                    format!("{}...", &prompt[..50])
                } else {
                    prompt.clone()
                };
                println!("    Prompt: {}", display);
            }
            if let Some(model) = &profile.model {
                println!("    Model: {}", model);
            }
            if !profile.parameters.is_empty() {
                println!("    Parameters: {}", serde_json::to_string(&profile.parameters).unwrap_or_default());
            }
        }
    }

    if !config.prompts.is_empty() {
        println!("\nPrompts:");
        for (name, prompt_entry) in &config.prompts {
            let preview = if prompt_entry.prompt.len() > 60 {
                format!("{}...", &prompt_entry.prompt[..60].replace('\n', " "))
            } else {
                prompt_entry.prompt.replace('\n', " ")
            };
            println!("  {}: {}", name, preview);
        }
    }

    println!("\nProviders:");
    for (name, provider_config) in &config.providers {
        println!("  {}:", name);
        if let Some(model) = &provider_config.default_model {
            println!("    Default model: {}", model);
        }
        if provider_config.api_key.is_some() {
            println!("    API key: (configured)");
        }
        if let Some(base_url) = &provider_config.base_url {
            println!("    Base URL: {}", base_url);
        }
        if !provider_config.parameters.is_empty() {
            println!("    Parameters: {}", serde_json::to_string(&provider_config.parameters).unwrap_or_default());
        }
    }
    Ok(())
}

/// Resolved settings from CLI, profile, and config
#[allow(dead_code)]
struct ResolvedSettings {
    provider_name: String,
    api_key: String,
    base_url: Option<String>,
    model: Option<String>,
    system_prompt: Option<String>,
    parameters: std::collections::HashMap<String, serde_json::Value>,
}

/// Resolve all settings from CLI args, profile, and config
fn resolve_settings(cli: &Cli, config: &Config) -> Result<ResolvedSettings> {
    // Determine which profile to use (CLI > config default)
    let profile_name = cli.profile.clone().unwrap_or_else(|| config.default_profile.clone());

    // Resolve profile (required)
    let resolved_profile = config.resolve_profile(&profile_name)
        .with_context(|| format!("Profile '{}' not found or missing provider", profile_name))?;

    // Provider comes from profile (CLI can override)
    let provider_name = cli
        .provider
        .clone()
        .unwrap_or(resolved_profile.provider_name.clone());

    // Get provider config
    let provider_config = config.providers.get(&provider_name);

    // Resolve API key
    let api_key = cli
        .base_url
        .as_ref()
        .and_then(|_| Some("none".to_string())) // If base_url provided via CLI, allow dummy key
        .or_else(|| provider_config.and_then(|p| p.api_key.clone()))
        .or_else(|| std::env::var(format!("{}_API_KEY", provider_name.to_uppercase())).ok())
        .with_context(|| {
            format!(
                "API key not found for provider '{}'. Configure in ~/.config/qq/config.toml",
                provider_name
            )
        })?;

    // Resolve base URL: CLI > provider config
    let base_url = cli
        .base_url
        .clone()
        .or_else(|| provider_config.and_then(|p| p.base_url.clone()));

    // Resolve model: CLI > profile > provider default
    let model = cli
        .model
        .clone()
        .or_else(|| resolved_profile.model.clone())
        .or_else(|| provider_config.and_then(|p| p.default_model.clone()));

    // Resolve system prompt: CLI > profile
    let system_prompt = cli
        .system
        .clone()
        .or(resolved_profile.system_prompt.clone());

    // Merge parameters: provider + profile (profile wins on conflicts)
    let mut parameters = provider_config
        .map(|p| p.parameters.clone())
        .unwrap_or_default();
    parameters.extend(resolved_profile.parameters.clone());

    Ok(ResolvedSettings {
        provider_name,
        api_key,
        base_url,
        model,
        system_prompt,
        parameters,
    })
}

fn create_provider_from_settings(settings: &ResolvedSettings) -> Result<Box<dyn Provider>> {
    let mut provider = OpenAIProvider::new(&settings.api_key);

    if let Some(model) = &settings.model {
        provider = provider.with_default_model(model);
    }
    if let Some(url) = &settings.base_url {
        provider = provider.with_base_url(url);
    }

    Ok(Box::new(provider))
}

// Keep old function for backwards compatibility with list_models
fn create_provider(cli: &Cli, config: &Config) -> Result<Box<dyn Provider>> {
    let settings = resolve_settings(cli, config)?;
    create_provider_from_settings(&settings)
}
