use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use qq_agents::{ProjectManagerAgent, InternalAgent};
use qq_core::{
    execute_tools_parallel_with_chunker, ChunkProcessor, CompletionRequest, Message, Provider,
    ToolRegistry,
};
use qq_providers::{AnthropicProvider, GeminiProvider, OpenAIProvider};

mod agents;
mod chat;
mod compaction;
mod config;
mod debug_log;
mod event_bus;
mod execution_context;
mod markdown;
mod setup;
mod tui;

pub use event_bus::AgentEventBus;
pub use execution_context::ExecutionContext;

use agents::{create_agent_tools, AgentExecutor, InformUserTool, DEFAULT_MAX_AGENT_DEPTH};
use config::{expand_path, AgentsConfig, Config};
use qq_core::{AgentMemory, ContextCompactor};
use qq_tools::bash::permissions::parse_config_overrides;

/// Log level for tracing output
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LogLevel {
    /// Most verbose: all tracing including LLM streaming chunks
    Trace,
    /// Verbose: LLM requests/responses, tool execution details
    Debug,
    /// Standard: high-level flow, iteration starts
    Info,
    /// Quiet: only warnings and errors
    Warn,
    /// Minimal: only errors
    Error,
}

impl LogLevel {
    fn as_filter(&self) -> &'static str {
        match self {
            LogLevel::Trace => "trace",
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        }
    }
}

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

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, value_enum, default_value = "warn")]
    pub log_level: LogLevel,

    /// Enable debug logging (shorthand for --log-level debug)
    #[arg(short, long)]
    pub debug: bool,

    /// Write conversation trace and debug logs to file (JSON-lines format)
    #[arg(long)]
    pub log_file: Option<std::path::PathBuf>,

    /// DEPRECATED: Use --log-file instead
    #[arg(long, hide = true)]
    pub debug_file: Option<std::path::PathBuf>,

    /// Disable all tools (for testing)
    #[arg(long)]
    pub no_tools: bool,

    /// Disable all agents (for testing)
    #[arg(long)]
    pub no_agents: bool,

    /// Minimal mode: no tools, no agents (for testing basic chat loop)
    #[arg(long)]
    pub minimal: bool,

    /// Use TUI mode (default for chat)
    #[arg(long, default_value = "true")]
    pub tui: bool,

    /// Disable TUI mode, use legacy readline interface
    #[arg(long)]
    pub no_tui: bool,

    /// Use built-in filesystem tools for search instead of bash (no bash tools)
    #[arg(long, conflicts_with = "insecure")]
    pub classic: bool,

    /// Allow bash tools without kernel sandbox isolation
    #[arg(long, conflicts_with = "classic")]
    pub insecure: bool,

    /// Primary agent to use for interactive sessions (overrides profile)
    /// Can be any internal agent: pm, explore, researcher, coder, reviewer, summarizer, planner, writer
    #[arg(short = 'A', long)]
    pub agent: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start interactive project management mode
    Manage {
        /// Initial system prompt for the conversation
        #[arg(short, long)]
        system: Option<String>,
    },
    /// List available profiles and their settings
    Profiles,
    /// Show current configuration
    Config,
    /// Initialize configuration files in ~/.config/qq
    Setup,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Determine if TUI mode will be used (needed for logging configuration)
    let will_use_tui = cli.tui && !cli.no_tui && atty::is(atty::Stream::Stdout)
        && cli.prompt.is_none(); // TUI only for chat mode, not completion mode

    // Resolve log level: --debug overrides --log-level
    let log_level = if cli.debug {
        LogLevel::Debug
    } else {
        cli.log_level
    };

    // Resolve log file: --log-file takes precedence over deprecated --debug-file
    let log_file = cli.log_file.as_ref().or(cli.debug_file.as_ref());

    // Set up logging
    let filter = EnvFilter::new(log_level.as_filter());

    if will_use_tui && log_file.is_none() {
        // TUI mode without log file: suppress all tracing output
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::sink)
            .init();
    } else if let Some(log_path) = log_file {
        // Log file specified: write JSON to file
        let file = std::fs::File::create(log_path)
            .with_context(|| format!("Failed to create log file: {:?}", log_path))?;
        tracing_subscriber::registry()
            .with(filter)
            .with(
                fmt::layer()
                    .json()
                    .with_writer(std::sync::Mutex::new(file))
            )
            .init();
    } else {
        // Non-TUI mode: write to stderr
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .init();
    }

    // Handle setup before config is required
    if matches!(&cli.command, Some(Commands::Setup)) {
        return setup::run();
    }

    // Load configuration (required for all other commands)
    let config = Config::load()?;

    match &cli.command {
        Some(Commands::Manage { system }) => {
            chat_mode(&cli, &config, system.clone()).await
        }
        Some(Commands::Profiles) => {
            list_profiles(&config)
        }
        Some(Commands::Config) => {
            show_config(&config)
        }
        Some(Commands::Setup) => unreachable!(),
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

/// Resources created by bash tool setup, passed to TUI/CLI for approval handling.
struct BashResources {
    mounts: Arc<qq_tools::SandboxMounts>,
    approval_rx: tokio::sync::mpsc::Receiver<qq_tools::ApprovalRequest>,
    permissions: Arc<qq_tools::PermissionStore>,
}

/// Build tools registry from config.
///
/// Bash tool modes:
/// - Default: requires kernel sandbox, exits if unavailable
/// - `--classic`: no bash tools, built-in search tools instead
/// - `--insecure`: bash tools without kernel sandbox
fn build_tools_registry(config: &Config, classic: bool, insecure: bool) -> Result<(ToolRegistry, Option<BashResources>)> {
    // Resolve root directory: config > $PWD
    let root = config.tools.root.as_ref()
        .map(|s| expand_path(s))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // Resolve memory db: config > default
    let memory_db = config.tools.memory_db.as_ref()
        .map(|s| expand_path(s))
        .or_else(|| Config::config_dir().ok().map(|d| d.join("memory.db")));

    let mut registry = ToolRegistry::new();

    // Determine whether bash tools will be available
    // --classic disables bash entirely; otherwise respect config
    let use_bash = !classic && config.tools.enable_bash;

    // If bash is requested (not classic, config enabled), verify sandbox unless --insecure
    if use_bash && !insecure {
        let executor = qq_tools::SandboxExecutor::detect();
        if !executor.supports_shell() {
            if is_apparmor_restricting_userns() {
                anyhow::bail!(
                    "Kernel sandbox unavailable — AppArmor is restricting user namespaces.\n\n\
                     To fix, run:  sudo ./scripts/setup-apparmor.sh\n\n\
                     Alternatively:\n  \
                     --classic   Use built-in search tools instead of bash\n  \
                     --insecure  Allow bash without kernel sandbox isolation"
                );
            } else {
                anyhow::bail!(
                    "Kernel sandbox unavailable — user namespaces are not supported.\n\n\
                     Alternatively:\n  \
                     --classic   Use built-in search tools instead of bash\n  \
                     --insecure  Allow bash without kernel sandbox isolation"
                );
            }
        }
    }

    // Create sandbox mounts early so we can share tmp_dir with filesystem tools.
    // Mounts are only created when bash is enabled with kernel sandbox (not --insecure).
    let mounts = if use_bash {
        if insecure {
            eprintln!("Warning: Running bash tools without kernel sandbox isolation (--insecure).");
        }

        let m = Arc::new(qq_tools::SandboxMounts::new(root.clone())
            .context("Failed to create per-instance /tmp directory")?);

        // Add configured extra mounts
        for mount_path in &config.tools.bash_mounts {
            let expanded = expand_path(mount_path);
            if expanded.exists() && expanded.is_dir() {
                m.add_mount(qq_tools::MountPoint {
                    host_path: expanded,
                    label: None,
                });
            } else {
                tracing::warn!(path = %mount_path, "Bash mount path does not exist or is not a directory");
            }
        }

        Some(m)
    } else {
        None
    };

    // Filesystem tools — include search tools only when bash is not available.
    // When kernel sandbox is active (not --insecure), share its /tmp with file tools.
    if config.tools.enable_filesystem {
        let mut fs_config = qq_tools::FileSystemConfig::new(&root)
            .with_write(config.tools.allow_write)
            .with_search_tools(!use_bash);
        if !insecure {
            if let Some(ref m) = mounts {
                fs_config = fs_config.with_sandbox_tmp(m.tmp_dir().to_path_buf());
            }
        }
        for tool in qq_tools::create_filesystem_tools_arc(fs_config) {
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
        for tool in qq_tools::create_preference_tools_arc(store) {
            registry.register(tool);
        }
    }

    // Web tools
    if config.tools.enable_web {
        let web_search_config = config.tools.web_search.as_ref().map(|ws| {
            qq_tools::WebSearchConfig::new(&ws.host, &ws.chat_model, &ws.embed_model)
        });
        for tool in qq_tools::create_web_tools_with_search(web_search_config) {
            registry.register(tool);
        }
    }

    // Bash tools — only when not in classic mode and config enables bash
    let bash_resources = if let Some(mounts) = mounts {
        // Build permission overrides from config
        let overrides = config.tools.bash_permissions.as_ref()
            .map(|p| parse_config_overrides(&p.session, &p.per_call, &p.restricted))
            .unwrap_or_default();
        let permissions = Arc::new(qq_tools::PermissionStore::new(overrides));

        let (approval_tx, approval_rx) = qq_tools::create_approval_channel();

        for tool in qq_tools::create_bash_tools(
            Arc::clone(&mounts),
            Arc::clone(&permissions),
            approval_tx,
        ) {
            registry.register(tool);
        }

        Some(BashResources {
            mounts,
            approval_rx,
            permissions,
        })
    } else {
        None
    };

    Ok((registry, bash_resources))
}

fn is_apparmor_restricting_userns() -> bool {
    std::fs::read_to_string("/proc/sys/kernel/apparmor_restrict_unprivileged_userns")
        .map(|s| s.trim() == "1")
        .unwrap_or(false)
}

async fn completion_mode(cli: &Cli, config: &Config, prompt: &str) -> Result<()> {
    // Resolve settings from profile, CLI, and config
    let settings = resolve_settings(cli, config)?;
    let provider: Arc<dyn Provider> = Arc::from(create_provider_from_settings(&settings)?);

    // Set up tools
    let (mut tools_registry, _bash_resources) = build_tools_registry(config, cli.classic, cli.insecure)?;

    // Set up chunk processor for large tool outputs
    let chunker_config = config.tools.chunker.to_chunker_config();
    let chunk_processor = ChunkProcessor::new(Arc::clone(&provider), chunker_config.clone());

    // Add process_large_data tool (requires provider)
    if chunker_config.enabled {
        tools_registry.register(qq_tools::create_process_data_tool_arc(
            Arc::clone(&provider),
            chunker_config,
        ));
    }

    let mut messages = Vec::new();

    // Add system prompt (CLI overrides profile)
    if let Some(system) = &settings.system_prompt {
        messages.push(Message::system(system.as_str()));
    }

    messages.push(Message::user(prompt));

    // Agentic loop - keep going while LLM returns tool calls
    let max_iterations = 100;
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
            // Print assistant message if any (content only, not thinking)
            let content = response.message.content.to_string_lossy();
            if !content.is_empty() {
                println!("{}", content);
            }

            // Add assistant message to history with empty content (don't store potential thinking)
            // Note: Some providers may leak thinking into content; we explicitly clear it
            let assistant_msg = Message::assistant_with_tool_calls("", response.message.tool_calls.clone());
            messages.push(assistant_msg);

            // Execute tools in parallel with chunking support
            let tool_calls = response.message.tool_calls.clone();

            for tool_call in &tool_calls {
                tracing::debug!(
                    tool = %tool_call.name,
                    arguments = %tool_call.arguments,
                    "Executing tool"
                );
            }

            let results = execute_tools_parallel_with_chunker(
                &tools_registry,
                tool_calls,
                Some(&chunk_processor),
                Some(prompt),
            )
            .await;

            for result in results {
                tracing::debug!(
                    tool_call_id = %result.tool_call_id,
                    result_len = result.content.len(),
                    is_error = result.is_error,
                    "Tool result"
                );
                tracing::trace!(
                    tool_call_id = %result.tool_call_id,
                    content = %result.content,
                    "Tool result content"
                );

                // Add tool result to messages
                messages.push(Message::tool_result(&result.tool_call_id, result.content));
            }

            // Continue loop to get next response
            continue;
        }

        // No tool calls - print final response and exit
        println!("{}", response.message.content.to_string_lossy());

        tracing::info!(
            prompt_tokens = response.usage.prompt_tokens,
            completion_tokens = response.usage.completion_tokens,
            total_tokens = response.usage.total_tokens,
            iterations = iteration + 1,
            "Completion finished"
        );

        return Ok(());
    }

    eprintln!("Warning: Max iterations ({}) reached", max_iterations);
    Ok(())
}

async fn chat_mode(cli: &Cli, config: &Config, system: Option<String>) -> Result<()> {
    // Resolve settings from profile, CLI, and config
    let settings = resolve_settings(cli, config)?;
    let provider: Arc<dyn Provider> = Arc::from(create_provider_from_settings(&settings)?);

    // Determine system prompt: explicit arg > CLI > profile
    let user_system_prompt = system
        .or_else(|| cli.system.clone())
        .or(settings.system_prompt.clone());

    // Check test mode flags
    let disable_tools = cli.no_tools || cli.minimal;
    let disable_agents = cli.no_agents || cli.minimal;

    // When using the PM agent, combine the PM agent's coordination prompt
    // with any user-specified prompt. The PM agent's prompt ensures proper
    // delegation behavior while the user's prompt can add additional context.
    let system_prompt = if settings.agent == "pm" || settings.agent == "chat" {
        let pm_agent = ProjectManagerAgent::new();
        let base_prompt = pm_agent.system_prompt();
        let preamble = qq_agents::generate_preamble(&qq_agents::PreambleContext {
            has_tools: true, // PM has task tracking tools
            has_sub_agents: !disable_agents,
            has_inform_user: true,
            has_task_tracking: false, // PM uses full task tools, not update_my_task
            has_preferences: false,   // PM delegates preference access to sub-agents
            has_bash: false,          // PM delegates bash to sub-agents
            is_read_only: false,
        });
        let combined = format!("{}\n\n---\n\n{}", preamble, base_prompt);
        match user_system_prompt {
            Some(user_prompt) => Some(format!(
                "{}\n\n---\n\n## Additional Instructions\n\n{}",
                combined, user_prompt
            )),
            None => Some(combined),
        }
    } else {
        user_system_prompt
    };

    // Set up base tools (conditionally)
    let (mut base_tools, bash_resources) = if disable_tools {
        tracing::debug!("Tools disabled (--no-tools or --minimal)");
        (ToolRegistry::new(), None)
    } else {
        build_tools_registry(config, cli.classic, cli.insecure)?
    };

    // Register task tracking tools (session-scoped, in-memory)
    let task_store = if !disable_tools {
        let store = std::sync::Arc::new(qq_tools::TaskStore::new());
        for tool in qq_tools::create_task_tools_arc(store.clone()) {
            base_tools.register(tool);
        }
        Some(store)
    } else {
        None
    };

    // Load agents config
    let agents_config = AgentsConfig::load().unwrap_or_default();

    // Create execution context for tracking agent/tool call stack
    let execution_context = ExecutionContext::new();

    // Determine whether to use TUI mode (needed early for event bus decision)
    let use_tui = cli.tui && !cli.no_tui && atty::is(atty::Stream::Stdout);

    // Set up debug logger if requested (log_file takes precedence over deprecated debug_file)
    let log_path = cli.log_file.as_ref().or(cli.debug_file.as_ref());
    let debug_logger: Option<Arc<debug_log::DebugLogger>> = if let Some(path) = log_path {
        match debug_log::DebugLogger::new(path) {
            Ok(logger) => {
                tracing::info!(path = %path.display(), "Writing structured debug log");
                Some(Arc::new(logger))
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to create debug log");
                None
            }
        }
    } else {
        None
    };

    // Create event bus for agent progress reporting (used in both TUI and readline modes)
    let mut event_bus = AgentEventBus::new(256);
    if let Some(ref logger) = debug_logger {
        event_bus = event_bus.with_debug_logger(Arc::clone(logger));
    }

    // Create scoped agent memory for persistent instance state
    let agent_memory = AgentMemory::new();

    // Create observational memory compactor (used by both ChatSession and agents)
    let observation_config = config
        .compaction
        .as_ref()
        .map(|c| c.to_observation_config())
        .unwrap_or_default();

    let compactor: Option<Arc<dyn ContextCompactor>> = {
        let compaction_provider = if let Some(ref comp_config) = config.compaction {
            if let Some(ref provider_name) = comp_config.provider {
                // Create a separate provider for compaction
                let comp_settings = resolve_settings_for_provider(provider_name, config)?;
                Arc::from(create_provider_from_settings(&comp_settings)?)
            } else {
                Arc::clone(&provider)
            }
        } else {
            Arc::clone(&provider)
        };

        let model_override = config
            .compaction
            .as_ref()
            .and_then(|c| c.model.clone());

        Some(Arc::new(compaction::LlmCompactor::new(
            compaction_provider,
            model_override,
        )))
    };

    // Create agent tools (conditionally)
    let agent_tools = if disable_agents || disable_tools {
        if disable_agents {
            tracing::debug!("Agents disabled (--no-agents or --minimal)");
        }
        vec![]
    } else {
        create_agent_tools(
            &base_tools,
            Arc::clone(&provider),
            &agents_config,
            &settings.agents,
            0, // Start at depth 0
            DEFAULT_MAX_AGENT_DEPTH,
            Some(execution_context.clone()),
            Some(event_bus.clone()),
            Some(agent_memory.clone()),
            "pm".to_string(),
            task_store.clone(),
            compactor.clone(),
        )
    };

    // Build the tools registry with base tools and agent tools
    let mut tools_registry = base_tools.clone();
    for tool in agent_tools {
        tools_registry.register(tool);
    }

    // Add inform_user tool for the main chat (allows primary agent to notify user)
    tools_registry.register(Arc::new(InformUserTool::new(
        event_bus.clone(),
        "assistant", // Primary agent name for main chat
    )));

    // Set up chunker config
    let chunker_config = config.tools.chunker.to_chunker_config();

    // Add process_large_data tool (requires provider)
    if chunker_config.enabled && !disable_tools {
        tools_registry.register(qq_tools::create_process_data_tool_arc(
            Arc::clone(&provider),
            chunker_config.clone(),
        ));
    }

    // Create executor for manual agent commands (@agent, /delegate)
    // Uses base_tools so manual commands also can't recurse
    let agent_executor = if disable_agents {
        None
    } else {
        let executor = AgentExecutor::new(
            Arc::clone(&provider),
            base_tools,
            agents_config,
            settings.agents.clone(),
        );
        Some(Arc::new(tokio::sync::RwLock::new(executor)))
    };

    // Destructure bash resources for TUI/CLI
    let (bash_mounts, bash_approval_rx, bash_permissions) = match bash_resources {
        Some(br) => (Some(br.mounts), Some(br.approval_rx), Some(br.permissions)),
        None => (None, None, None),
    };

    if use_tui {
        tui::run_tui(
            cli,
            config,
            provider,
            system_prompt,
            tools_registry,
            settings.parameters,
            settings.profile_name,
            settings.agent.clone(),
            agent_executor,
            execution_context,
            chunker_config,
            Some(event_bus),
            debug_logger,
            agent_memory.clone(),
            bash_mounts.clone(),
            bash_approval_rx,
            bash_permissions.clone(),
            task_store.clone(),
            compactor.clone(),
            observation_config.clone(),
        )
        .await
    } else {
        chat::run_chat(
            cli,
            config,
            provider,
            system_prompt,
            tools_registry,
            settings.parameters,
            settings.model,
            agent_executor,
            chunker_config,
            event_bus,
            debug_logger,
            agent_memory.clone(),
            bash_mounts,
            bash_approval_rx,
            bash_permissions,
            task_store,
            compactor,
            observation_config,
        )
        .await
    }
}

fn list_profiles(config: &Config) -> Result<()> {
    if config.profiles.is_empty() {
        println!("No profiles configured.");
        return Ok(());
    }

    println!("Available profiles:\n");

    // Sort profile names, but put default first
    let mut profile_names: Vec<_> = config.profiles.keys().collect();
    profile_names.sort();

    for name in profile_names {
        let is_default = name == &config.default_profile;
        let marker = if is_default { " (default)" } else { "" };

        println!("  {}{}", name, marker);

        if let Some(resolved) = config.resolve_profile(name) {
            // Provider and model
            let model_info = resolved
                .model
                .or_else(|| {
                    resolved
                        .provider_config
                        .as_ref()
                        .and_then(|p| p.default_model.clone())
                })
                .unwrap_or_else(|| "default".to_string());

            println!("    Provider: {}", resolved.provider_name);
            println!("    Model: {}", model_info);

            // Base URL if custom
            if let Some(ref pc) = resolved.provider_config {
                if let Some(ref url) = pc.base_url {
                    println!("    Base URL: {}", url);
                }
            }

            // System prompt (truncated)
            if let Some(ref prompt) = resolved.system_prompt {
                let preview = if prompt.len() > 60 {
                    format!("{}...", prompt[..60].replace('\n', " "))
                } else {
                    prompt.replace('\n', " ")
                };
                println!("    System: {}", preview);
            }

            // Parameters (if any)
            if !resolved.parameters.is_empty() {
                let params: Vec<String> = resolved
                    .parameters
                    .iter()
                    .map(|(k, v)| {
                        let val = match v {
                            serde_json::Value::String(s) => s.clone(),
                            _ => v.to_string(),
                        };
                        format!("{}={}", k, val)
                    })
                    .collect();
                println!("    Parameters: {}", params.join(", "));
            }

            // Primary agent
            if resolved.agent != "pm" && resolved.agent != "chat" {
                println!("    Primary agent: {}", resolved.agent);
            }
        } else {
            println!("    (invalid - missing provider)");
        }

        println!();
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
    profile_name: String,
    provider_name: String,
    provider_type: String,
    api_key: String,
    base_url: Option<String>,
    model: Option<String>,
    system_prompt: Option<String>,
    parameters: std::collections::HashMap<String, serde_json::Value>,
    agents: Option<Vec<String>>,
    /// Primary agent for interactive sessions
    agent: String,
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
        .map(|_| "none".to_string()) // If base_url provided via CLI, allow dummy key
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

    // Resolve primary agent: CLI > profile > default
    let agent = cli
        .agent
        .clone()
        .unwrap_or_else(|| resolved_profile.agent.clone());

    // Resolve provider type
    let provider_type = resolve_provider_type(
        resolved_profile.provider_type.as_deref(),
        &provider_name,
        base_url.as_deref(),
    );

    Ok(ResolvedSettings {
        profile_name,
        provider_name,
        provider_type,
        api_key,
        base_url,
        model,
        system_prompt,
        parameters,
        agents: resolved_profile.agents.clone(),
        agent,
    })
}

/// Resolve the provider type from explicit config, provider name, or base_url.
///
/// Priority:
/// 1. Explicit `type` in provider config always wins
/// 2. If no type but base_url is set → "openai" (OpenAI-compatible mode)
/// 3. If no type and no base_url → infer from provider name
fn resolve_provider_type(
    explicit_type: Option<&str>,
    provider_name: &str,
    base_url: Option<&str>,
) -> String {
    // 1. Explicit type always wins
    if let Some(t) = explicit_type {
        return t.to_lowercase();
    }

    // 2. If base_url is set, default to openai-compatible
    if base_url.is_some() {
        return "openai".to_string();
    }

    // 3. Infer from provider name
    let name = provider_name.to_lowercase();
    match name.as_str() {
        "anthropic" | "claude" => "anthropic".to_string(),
        "gemini" | "google" => "gemini".to_string(),
        _ => "openai".to_string(),
    }
}

/// Resolve minimal settings for a provider by name (used for compaction provider override).
fn resolve_settings_for_provider(provider_name: &str, config: &Config) -> Result<ResolvedSettings> {
    let provider_config = config.providers.get(provider_name)
        .with_context(|| format!("Compaction provider '{}' not found in config", provider_name))?;

    let api_key = provider_config.api_key.clone()
        .or_else(|| std::env::var(format!("{}_API_KEY", provider_name.to_uppercase())).ok())
        .with_context(|| format!("API key not found for compaction provider '{}'", provider_name))?;

    let provider_type = resolve_provider_type(
        provider_config.provider_type.as_deref(),
        provider_name,
        provider_config.base_url.as_deref(),
    );

    Ok(ResolvedSettings {
        profile_name: String::new(),
        provider_name: provider_name.to_string(),
        provider_type,
        api_key,
        base_url: provider_config.base_url.clone(),
        model: provider_config.default_model.clone(),
        system_prompt: None,
        parameters: provider_config.parameters.clone(),
        agents: None,
        agent: String::new(),
    })
}

fn create_provider_from_settings(settings: &ResolvedSettings) -> Result<Box<dyn Provider>> {
    match settings.provider_type.as_str() {
        "anthropic" => {
            let mut provider = AnthropicProvider::new(&settings.api_key);
            if let Some(model) = &settings.model {
                provider = provider.with_default_model(model);
            }
            if let Some(url) = &settings.base_url {
                provider = provider.with_base_url(url);
            }
            Ok(Box::new(provider))
        }
        "gemini" => {
            let mut provider = GeminiProvider::new(&settings.api_key);
            if let Some(model) = &settings.model {
                provider = provider.with_default_model(model);
            }
            if let Some(url) = &settings.base_url {
                provider = provider.with_base_url(url);
            }
            Ok(Box::new(provider))
        }
        _ => {
            // Default: OpenAI-compatible
            let mut provider = OpenAIProvider::new(&settings.api_key);
            if let Some(model) = &settings.model {
                provider = provider.with_default_model(model);
            }
            if let Some(url) = &settings.base_url {
                provider = provider.with_base_url(url);
            }
            Ok(Box::new(provider))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_provider_type_explicit() {
        assert_eq!(resolve_provider_type(Some("anthropic"), "whatever", None), "anthropic");
        assert_eq!(resolve_provider_type(Some("gemini"), "whatever", None), "gemini");
        assert_eq!(resolve_provider_type(Some("openai"), "whatever", None), "openai");
        // Explicit type wins even with base_url
        assert_eq!(
            resolve_provider_type(Some("anthropic"), "openai", Some("http://localhost:8080")),
            "anthropic"
        );
    }

    #[test]
    fn test_resolve_provider_type_base_url_defaults_openai() {
        assert_eq!(
            resolve_provider_type(None, "custom", Some("http://localhost:11434/v1")),
            "openai"
        );
        assert_eq!(
            resolve_provider_type(None, "anthropic", Some("http://proxy.example.com")),
            "openai"
        );
    }

    #[test]
    fn test_resolve_provider_type_name_inference() {
        assert_eq!(resolve_provider_type(None, "anthropic", None), "anthropic");
        assert_eq!(resolve_provider_type(None, "claude", None), "anthropic");
        assert_eq!(resolve_provider_type(None, "Anthropic", None), "anthropic");
        assert_eq!(resolve_provider_type(None, "gemini", None), "gemini");
        assert_eq!(resolve_provider_type(None, "google", None), "gemini");
        assert_eq!(resolve_provider_type(None, "Google", None), "gemini");
        assert_eq!(resolve_provider_type(None, "openai", None), "openai");
        assert_eq!(resolve_provider_type(None, "groq", None), "openai");
        assert_eq!(resolve_provider_type(None, "ollama", None), "openai");
    }
}

