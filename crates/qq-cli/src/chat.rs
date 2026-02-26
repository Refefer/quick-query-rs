//! Interactive chat mode with readline support.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use crossterm::ExecutableCommand;
use rustyline::error::ReadlineError;
use rustyline::history::FileHistory;
use rustyline::{Config, Editor};
use tokio::sync::RwLock;

use futures::StreamExt;

use qq_core::{
    execute_tools_parallel_with_chunker, AgentMemory, ChunkProcessor, ChunkerConfig,
    CompletionRequest, ContextCompactor, Message, ObservationConfig, ObservationalMemory,
    Provider, StreamChunk, ToolCall, ToolRegistry,
};

use crate::agents::AgentExecutor;
use crate::config;
use crate::config::Config as AppConfig;
use crate::debug_log::DebugLogger;
use crate::event_bus::{AgentEvent, AgentEventBus};
use crate::markdown::MarkdownRenderer;
use crate::Cli;

/// Chat session state with observational memory compaction.
pub struct ChatSession {
    pub messages: Vec<Message>,
    pub system_prompt: Option<String>,
    compactor: Option<Arc<dyn ContextCompactor>>,
    pub observation_memory: ObservationalMemory,
}

impl ChatSession {
    pub fn new(system_prompt: Option<String>) -> Self {
        Self {
            messages: Vec::new(),
            system_prompt,
            compactor: None,
            observation_memory: ObservationalMemory::new(ObservationConfig::default()),
        }
    }

    /// Set the context compactor for LLM-powered observation and reflection.
    pub fn with_compactor(mut self, compactor: Arc<dyn ContextCompactor>) -> Self {
        self.compactor = Some(compactor);
        self
    }

    /// Set the observation config (thresholds, preserve_recent, etc.).
    pub fn with_observation_config(mut self, config: ObservationConfig) -> Self {
        self.observation_memory = ObservationalMemory::new(config);
        self
    }

    pub fn add_user_message(&mut self, content: &str) {
        self.messages.push(Message::user(content));
    }

    pub fn add_assistant_message(&mut self, content: &str) {
        self.messages.push(Message::assistant(content));
    }

    pub fn add_assistant_with_tools(&mut self, message: Message) {
        self.messages.push(message);
    }

    pub fn add_tool_result(&mut self, tool_call_id: &str, result: &str) {
        self.messages.push(Message::tool_result(tool_call_id, result));
    }

    pub fn build_messages(&self) -> Vec<Message> {
        let mut msgs = Vec::new();

        // Merge system prompt and observation log into a single system message
        // to avoid multi-system-message errors with strict chat templates.
        let log = self.observation_memory.observation_log();
        let has_system = self.system_prompt.is_some();
        let has_log = !log.is_empty();

        if has_log {
            tracing::debug!(
                observation_log_bytes = log.len(),
                recent_messages = self.messages.len(),
                "Building messages with observation log"
            );
        }

        if has_system || has_log {
            let mut system_content = self.system_prompt.clone().unwrap_or_default();
            if has_log {
                if !system_content.is_empty() {
                    system_content.push_str("\n\n");
                }
                system_content.push_str(&format!(
                    "## Observation Log\n\n\
                     The following is a structured log of observations from earlier in this \
                     conversation. Each entry captures a specific event, decision, or finding.\n\n\
                     {}",
                    log
                ));
            }
            msgs.push(Message::system(system_content.as_str()));
        }

        // Recent unobserved messages
        msgs.extend(self.messages.clone());
        msgs
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.observation_memory.clear();
    }

    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Get the current observation count (for detecting compaction).
    pub fn observation_count(&self) -> u32 {
        self.observation_memory.observation_count
    }

    /// Calculate total byte count of all messages plus observation log.
    pub fn total_bytes(&self) -> usize {
        let msg_bytes: usize = self.messages.iter().map(|m| m.byte_count()).sum();
        msg_bytes + self.observation_memory.log_bytes()
    }

    /// Compact the conversation history using observational memory.
    pub async fn compact_if_needed(&mut self) {
        tracing::debug!(
            message_count = self.messages.len(),
            total_bytes = self.total_bytes(),
            log_bytes = self.observation_memory.log_bytes(),
            observation_count = self.observation_memory.observation_count,
            reflection_count = self.observation_memory.reflection_count,
            "Checking if compaction needed"
        );

        if let Some(ref compactor) = self.compactor {
            if let Err(e) = self
                .observation_memory
                .compact(&mut self.messages, compactor.as_ref())
                .await
            {
                tracing::error!(error = %e, "Observation memory compaction failed");
            }
        }
    }
}

/// Get the process RSS (Resident Set Size) in bytes.
/// Returns None on non-Linux platforms.
pub fn get_rss_bytes() -> Option<usize> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(statm) = std::fs::read_to_string("/proc/self/statm") {
            // Second field is RSS in pages
            if let Some(rss_pages) = statm.split_whitespace().nth(1) {
                if let Ok(pages) = rss_pages.parse::<usize>() {
                    let page_size = 4096; // standard page size
                    return Some(pages * page_size);
                }
            }
        }
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Format bytes in a human-readable way.
pub fn format_bytes(bytes: usize) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

/// Chat commands
enum ChatCommand {
    Quit,
    Clear,
    Reset,
    History,
    Help,
    Tools,
    Agents,
    Memory,
    Mount(String),
    Mounts,
    Delegate { agent: String, task: String },
    AgentCall { agent: String, task: String }, // @agent syntax
    System(String),
    Debug(String), // /debug subcommand
    None(String),  // Regular message
}

fn parse_command(input: &str) -> ChatCommand {
    let trimmed = input.trim();

    if trimmed.is_empty() {
        return ChatCommand::None(String::new());
    }

    // Check for @agent syntax: @agent_name <task>
    if let Some(stripped) = trimmed.strip_prefix('@') {
        let parts: Vec<&str> = stripped.splitn(2, char::is_whitespace).collect();
        let agent = parts[0].to_string();
        let task = parts.get(1).map(|s| s.trim().to_string()).unwrap_or_default();

        if task.is_empty() {
            eprintln!("Usage: @{} <task>", agent);
            return ChatCommand::None(String::new());
        }

        return ChatCommand::AgentCall { agent, task };
    }

    if !trimmed.starts_with('/') {
        return ChatCommand::None(trimmed.to_string());
    }

    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    let cmd = parts[0].to_lowercase();
    let arg = parts.get(1).map(|s| s.to_string()).unwrap_or_default();

    match cmd.as_str() {
        "/quit" | "/exit" | "/q" => ChatCommand::Quit,
        "/clear" | "/c" => ChatCommand::Clear,
        "/reset" => ChatCommand::Reset,
        "/history" | "/h" => ChatCommand::History,
        "/help" | "/?" => ChatCommand::Help,
        "/tools" | "/t" => ChatCommand::Tools,
        "/agents" | "/a" => ChatCommand::Agents,
        "/delegate" | "/d" => {
            // Parse: /delegate agent_name <task>
            let delegate_parts: Vec<&str> = arg.splitn(2, char::is_whitespace).collect();
            if delegate_parts.is_empty() || delegate_parts[0].is_empty() {
                eprintln!("Usage: /delegate <agent> <task>");
                ChatCommand::None(String::new())
            } else {
                let agent = delegate_parts[0].to_string();
                let task = delegate_parts.get(1).map(|s| s.trim().to_string()).unwrap_or_default();

                if task.is_empty() {
                    eprintln!("Usage: /delegate {} <task>", agent);
                    ChatCommand::None(String::new())
                } else {
                    ChatCommand::Delegate { agent, task }
                }
            }
        }
        "/memory" | "/mem" => ChatCommand::Memory,
        "/mount" => ChatCommand::Mount(arg),
        "/mounts" => ChatCommand::Mounts,
        "/system" | "/sys" => ChatCommand::System(arg),
        "/debug" => ChatCommand::Debug(arg),
        _ => {
            eprintln!("Unknown command: {}. Type /help for available commands.", cmd);
            ChatCommand::None(String::new())
        }
    }
}

fn print_help() {
    println!(
        r#"
Chat Commands:
  /help, /?           Show this help message
  /quit, /exit        Exit chat mode
  /clear, /c          Clear conversation + reset counters
  /reset              Full reset (clear + agent memory + tasks)
  /history, /h        Show message count
  /memory, /mem       Show memory usage diagnostics
  /tools, /t          List available tools
  /agents, /a         List available agents
  /delegate <a> <t>   Delegate task <t> to agent <a>
  /mount <path>       Add read-only mount to bash sandbox
  /mounts             List current bash sandbox mounts
  /system <msg>       Set a new system prompt
  /debug <subcmd>     Debug commands (messages, count, dump)

Agents:
  @agent <task>       Quick agent invocation (e.g., @explore Find all tests)

Debug subcommands:
  /debug messages     Show all messages with role and content preview
  /debug count        Show message counts by role
  /debug dump <file>  Dump messages to a JSON file

Tips:
  - Press Ctrl+C to cancel current generation
  - Press Ctrl+D to exit
  - Up/Down arrows navigate history
  - Use --debug-file <path> to enable file logging
  - Use --minimal for testing basic chat (no tools/agents)
"#
    );
}

/// Print a section header with styling (full-width horizontal rule with centered title)
fn print_section_header(title: &str) -> std::io::Result<()> {
    use crossterm::style::{Color, SetForegroundColor, ResetColor};
    use crossterm::terminal::size;
    use std::io::Write;

    let width = size().map(|(w, _)| w as usize).unwrap_or(80);
    let title_len = title.len() + 2; // title + spaces on each side
    let remaining = width.saturating_sub(title_len).saturating_sub(1);
    let left_len = remaining / 2;
    let right_len = remaining - left_len;

    let left_rule = "─".repeat(left_len);
    let right_rule = "─".repeat(right_len);

    let mut stdout = std::io::stdout();
    stdout.execute(SetForegroundColor(Color::DarkGrey))?;
    print!("{} ", left_rule);
    stdout.execute(SetForegroundColor(Color::Cyan))?;
    print!("{}", title);
    stdout.execute(SetForegroundColor(Color::DarkGrey))?;
    println!(" {}", right_rule);
    stdout.execute(ResetColor)?;
    stdout.flush()?;
    Ok(())
}

/// Format tool call arguments for display, truncating long values
fn format_tool_args(args: &serde_json::Value) -> String {
    match args {
        serde_json::Value::Object(map) => {
            let parts: Vec<String> = map
                .iter()
                .map(|(k, v)| {
                    let val = match v {
                        serde_json::Value::String(s) => {
                            if s.len() > 50 {
                                format!("\"{}...\"", &s[..47])
                            } else {
                                format!("\"{}\"", s)
                            }
                        }
                        serde_json::Value::Array(arr) => {
                            if arr.len() > 3 {
                                format!("[{} items]", arr.len())
                            } else {
                                let items: Vec<String> = arr.iter().map(|v| {
                                    let s = v.to_string();
                                    if s.len() > 20 { format!("{}...", &s[..17]) } else { s }
                                }).collect();
                                format!("[{}]", items.join(", "))
                            }
                        }
                        other => {
                            let s = other.to_string();
                            if s.len() > 50 {
                                format!("{}...", &s[..47])
                            } else {
                                s
                            }
                        }
                    };
                    format!("{}={}", k, val)
                })
                .collect();
            parts.join(", ")
        }
        serde_json::Value::Null => String::new(),
        other => {
            let s = other.to_string();
            if s.len() > 100 {
                format!("{}...", &s[..97])
            } else {
                s
            }
        }
    }
}

/// Print a tool call notification
fn print_tool_call(name: &str, args: &serde_json::Value) -> std::io::Result<()> {
    use crossterm::style::{Color, SetForegroundColor, ResetColor};
    use std::io::Write;

    let formatted_args = format_tool_args(args);

    let mut stdout = std::io::stdout();
    stdout.execute(SetForegroundColor(Color::DarkGrey))?;
    print!("▶ ");
    stdout.execute(SetForegroundColor(Color::Yellow))?;
    print!("{}", name);
    if !formatted_args.is_empty() {
        stdout.execute(SetForegroundColor(Color::DarkGrey))?;
        print!(" {}", formatted_args);
    }
    println!();
    stdout.execute(ResetColor)?;
    stdout.flush()?;
    Ok(())
}

/// Print prompt hint showing available commands
fn print_prompt_hint() -> std::io::Result<()> {
    use crossterm::style::{Color, SetForegroundColor, ResetColor};
    use std::io::Write;

    let mut stdout = std::io::stdout();
    stdout.execute(SetForegroundColor(Color::DarkGrey))?;
    println!("/help · /quit or Ctrl+D · Ctrl+C to interrupt");
    stdout.execute(ResetColor)?;
    stdout.flush()?;
    Ok(())
}

/// Handle debug subcommands
fn handle_debug_command(subcmd: &str, session: &ChatSession) {
    let parts: Vec<&str> = subcmd.splitn(2, ' ').collect();
    let cmd = parts.first().map(|s| s.trim()).unwrap_or("");
    let arg = parts.get(1).map(|s| s.trim()).unwrap_or("");

    match cmd {
        "messages" | "m" => {
            println!("\n=== Message History ({} messages) ===\n", session.messages.len());
            for (i, msg) in session.messages.iter().enumerate() {
                let content = msg.content.to_string_lossy();
                let preview = if content.len() > 80 {
                    format!("{}...", &content[..80].replace('\n', " "))
                } else {
                    content.replace('\n', " ")
                };
                let tool_info = if !msg.tool_calls.is_empty() {
                    format!(" [+{} tool calls]", msg.tool_calls.len())
                } else if msg.tool_call_id.is_some() {
                    " [tool result]".to_string()
                } else {
                    String::new()
                };
                println!("[{}] {} ({} chars){}: {}", i, msg.role, msg.content.to_string_lossy().len(), tool_info, preview);
            }
            println!();
        }
        "count" | "c" => {
            let mut counts = std::collections::HashMap::new();
            for msg in &session.messages {
                *counts.entry(msg.role.to_string()).or_insert(0) += 1;
            }
            println!("\n=== Message Counts ===");
            println!("  Total: {}", session.messages.len());
            for (role, count) in &counts {
                println!("  {}: {}", role, count);
            }
            println!();
        }
        "dump" => {
            if arg.is_empty() {
                eprintln!("Usage: /debug dump <filename>");
                return;
            }
            match dump_messages_to_file(&session.messages, arg) {
                Ok(_) => println!("Messages dumped to {}", arg),
                Err(e) => eprintln!("Failed to dump messages: {}", e),
            }
        }
        "" => {
            eprintln!("Debug subcommands: messages, count, dump <file>");
            eprintln!("Type /help for more information.");
        }
        _ => {
            eprintln!("Unknown debug subcommand: {}. Use: messages, count, dump", cmd);
        }
    }
}

/// Dump messages to a JSON file for analysis
fn dump_messages_to_file(messages: &[Message], filename: &str) -> std::io::Result<()> {
    use std::fs::File;
    use std::io::Write;

    let json = serde_json::to_string_pretty(messages)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let mut file = File::create(filename)?;
    file.write_all(json.as_bytes())?;
    Ok(())
}

/// Run interactive chat mode
#[allow(clippy::too_many_arguments)]
pub async fn run_chat(
    cli: &Cli,
    _config: &AppConfig,
    provider: Arc<dyn Provider>,
    system_prompt: Option<String>,
    tools_registry: ToolRegistry,
    extra_params: std::collections::HashMap<String, serde_json::Value>,
    model: Option<String>,
    agent_executor: Option<Arc<RwLock<AgentExecutor>>>,
    chunker_config: ChunkerConfig,
    event_bus: AgentEventBus,
    debug_logger: Option<Arc<DebugLogger>>,
    agent_memory: AgentMemory,
    bash_mounts: Option<Arc<qq_tools::SandboxMounts>>,
    bash_approval_rx: Option<tokio::sync::mpsc::Receiver<qq_tools::ApprovalRequest>>,
    _bash_permissions: Option<Arc<qq_tools::PermissionStore>>,
    task_store: Option<Arc<qq_tools::TaskStore>>,
    compactor: Option<Arc<dyn ContextCompactor>>,
    observation_config: ObservationConfig,
) -> Result<()> {
    // Create chunk processor for large tool outputs
    let chunk_processor = ChunkProcessor::new(Arc::clone(&provider), chunker_config);

    // Subscribe to event bus for agent notifications
    let mut event_rx = event_bus.subscribe();
    tokio::spawn(async move {
        while let Ok(event) = event_rx.recv().await {
            if let AgentEvent::UserNotification { agent_name, message } = event {
                // Print notification to stdout with visual distinction
                println!("\n> [{}] {}", agent_name, message);
            }
        }
    });

    // Spawn approval handler for bash tool (handles per-call command approval)
    if let Some(mut approval_rx) = bash_approval_rx {
        tokio::spawn(async move {
            while let Some(request) = approval_rx.recv().await {
                eprintln!("\n--- Bash approval required ---");
                eprintln!("  Command: {}", request.full_command);
                eprintln!(
                    "  Requires approval: {}",
                    request.trigger_commands.join(", ")
                );
                eprintln!("  [a]llow once / allow for [s]ession / [d]eny (default: deny)");
                eprint!("  > ");

                let mut input = String::new();
                match std::io::stdin().read_line(&mut input) {
                    Ok(_) => {
                        let response = match input.trim().to_lowercase().as_str() {
                            "a" | "allow" => qq_tools::ApprovalResponse::Allow,
                            "s" | "session" => qq_tools::ApprovalResponse::AllowForSession,
                            _ => qq_tools::ApprovalResponse::Deny,
                        };
                        let _ = request.response_tx.send(response);
                    }
                    Err(_) => {
                        let _ = request.response_tx.send(qq_tools::ApprovalResponse::Deny);
                    }
                }
            }
        });
    }

    // Set up readline with history
    let config = Config::builder()
        .history_ignore_space(true)
        .history_ignore_dups(true)?
        .build();

    let history_path = get_history_path();
    let mut rl: Editor<(), FileHistory> = Editor::with_config(config)?;

    // Load history if available
    if let Some(path) = &history_path {
        let _ = rl.load_history(path);
    }

    let mut session = ChatSession::new(system_prompt)
        .with_observation_config(observation_config);
    if let Some(compactor) = compactor {
        session = session.with_compactor(compactor);
    }

    // Log conversation start
    if let Some(ref logger) = debug_logger {
        logger.log_conversation_start(
            session.system_prompt.as_deref(),
            model.as_deref(),
        );
    }

    loop {
        // Print hint line before prompt
        print_prompt_hint()?;

        match rl.readline("you> ") {
            Ok(line) => {
                // Add to readline history
                let _ = rl.add_history_entry(&line);

                match parse_command(&line) {
                    ChatCommand::Quit => {
                        println!("Goodbye!");
                        break;
                    }
                    ChatCommand::Clear => {
                        session.clear();
                        println!("Conversation cleared.\n");
                    }
                    ChatCommand::Reset => {
                        session.clear();
                        agent_memory.clear_all().await;
                        if let Some(ref ts) = task_store {
                            ts.clear();
                        }
                        println!("Session reset (conversation + agent memory + tasks cleared).\n");
                    }
                    ChatCommand::History => {
                        println!(
                            "Messages in conversation: {} ({} user + assistant turns)\n",
                            session.message_count(),
                            session.message_count() / 2
                        );
                    }
                    ChatCommand::Help => {
                        print_help();
                    }
                    ChatCommand::Memory => {
                        println!("\n=== Memory Usage ===");
                        println!("  Messages (recent): {}", session.message_count());
                        println!("  Message bytes:     {}", format_bytes(
                            session.messages.iter().map(|m| m.byte_count()).sum::<usize>()
                        ));
                        println!("  Observation log:   {}", format_bytes(session.observation_memory.log_bytes()));
                        println!("  Total context:     {}", format_bytes(session.total_bytes()));
                        println!("  Observations:      {}", session.observation_memory.observation_count);
                        println!("  Reflections:       {}", session.observation_memory.reflection_count);
                        if let Some(rss) = get_rss_bytes() {
                            println!("  Process RSS:       {}", format_bytes(rss));
                        }

                        let diagnostics = agent_memory.diagnostics().await;
                        if !diagnostics.is_empty() {
                            println!("\n  Agent Instance Memory:");
                            for (scope, bytes, calls) in &diagnostics {
                                println!(
                                    "    {:<30} {}  ({} calls)",
                                    scope,
                                    format_bytes(*bytes),
                                    calls
                                );
                            }
                        }
                        println!();
                    }
                    ChatCommand::Mount(path_str) => {
                        if path_str.is_empty() {
                            println!("Usage: /mount <path>");
                        } else if let Some(ref mounts) = bash_mounts {
                            let expanded = config::expand_path(&path_str);
                            if !expanded.exists() {
                                println!("Path does not exist: {}", expanded.display());
                            } else if !expanded.is_dir() {
                                println!("Path is not a directory: {}", expanded.display());
                            } else {
                                match expanded.canonicalize() {
                                    Ok(canonical) => {
                                        mounts.add_mount(qq_tools::MountPoint {
                                            host_path: canonical.clone(),
                                            label: None,
                                        });
                                        println!("Mount added: {} (read-only)", canonical.display());
                                    }
                                    Err(e) => println!("Failed to resolve path: {}", e),
                                }
                            }
                        } else {
                            println!("Bash tools are disabled.");
                        }
                    }
                    ChatCommand::Mounts => {
                        if let Some(ref mounts) = bash_mounts {
                            println!("\nBash sandbox mounts:");
                            println!("{}", mounts.format_mounts());
                            println!();
                        } else {
                            println!("Bash tools are disabled.");
                        }
                    }
                    ChatCommand::Tools => {
                        println!("\nAvailable tools:");
                        let mut names: Vec<_> = tools_registry.names().into_iter().collect();
                        names.sort();
                        for name in names {
                            if let Some(tool) = tools_registry.get(name) {
                                println!("  {} - {}", name, tool.description());
                            }
                        }
                        println!();
                    }
                    ChatCommand::Agents => {
                        if let Some(ref executor) = agent_executor {
                            let exec = executor.read().await;
                            let agents = exec.list_agents();
                            if agents.is_empty() {
                                println!("\nNo agents available (all disabled in profile).\n");
                            } else {
                                println!("\nAvailable agents (LLM can use these automatically as tools):");
                                for agent in agents {
                                    let type_marker = if agent.is_internal { "(built-in)" } else { "(external)" };
                                    println!("  Agent[{}] {} - {}", agent.name, type_marker, agent.description);
                                    if !agent.tools.is_empty() {
                                        println!("    Agent tools: {}", agent.tools.join(", "));
                                    }
                                }
                                println!("\nManual invocation: @agent <task> or /delegate <agent> <task>\n");
                            }
                        } else {
                            println!("\nAgents are not configured.\n");
                        }
                    }
                    ChatCommand::Delegate { agent, task } | ChatCommand::AgentCall { agent, task } => {
                        if let Some(ref executor) = agent_executor {
                            let exec = executor.read().await;
                            if !exec.has_agent(&agent) {
                                eprintln!("Unknown or disabled agent: {}. Use /agents to list available agents.\n", agent);
                                continue;
                            }

                            print_section_header(&format!("Agent: {}", agent))?;

                            match exec.run(&agent, &task).await {
                                Ok(response) => {
                                    // Create a markdown renderer for the agent output
                                    let mut renderer = MarkdownRenderer::new();
                                    renderer.push(&response)?;
                                    renderer.finish()?;
                                    println!();
                                }
                                Err(e) => {
                                    eprintln!("\nAgent error: {}\n", e);
                                }
                            }
                        } else {
                            eprintln!("\nAgents are not configured. Ensure your profile supports agents.\n");
                        }
                    }
                    ChatCommand::System(new_system) => {
                        if new_system.is_empty() {
                            if let Some(sys) = &session.system_prompt {
                                println!("Current system prompt: {}\n", sys);
                            } else {
                                println!("No system prompt set.\n");
                            }
                        } else {
                            session.system_prompt = Some(new_system.clone());
                            println!("System prompt updated.\n");
                        }
                    }
                    ChatCommand::Debug(subcmd) => {
                        handle_debug_command(&subcmd, &session);
                    }
                    ChatCommand::None(text) => {
                        if text.is_empty() {
                            continue;
                        }

                        session.add_user_message(&text);

                        // Log user message
                        if let Some(ref logger) = debug_logger {
                            logger.log_user_message(&text);
                        }

                        // Run completion loop
                        match run_completion(
                            cli,
                            &provider,
                            &mut session,
                            &tools_registry,
                            &extra_params,
                            &model,
                            debug_logger.as_ref(),
                            &chunk_processor,
                            &text,
                        )
                        .await
                        {
                            Ok(_) => {}
                            Err(e) => {
                                eprintln!("\nError: {}\n", e);
                                // Remove the failed user message
                                session.messages.pop();
                            }
                        }
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("^C");
                continue;
            }
            Err(ReadlineError::Eof) => {
                println!("Goodbye!");
                break;
            }
            Err(e) => {
                eprintln!("Error reading input: {}", e);
                break;
            }
        }
    }

    // Save history
    if let Some(path) = &history_path {
        let _ = rl.save_history(path);
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_completion(
    cli: &Cli,
    provider: &Arc<dyn Provider>,
    session: &mut ChatSession,
    tools_registry: &ToolRegistry,
    extra_params: &std::collections::HashMap<String, serde_json::Value>,
    model: &Option<String>,
    debug_logger: Option<&Arc<DebugLogger>>,
    chunk_processor: &ChunkProcessor,
    original_query: &str,
) -> Result<()> {
    let include_tool_reasoning = provider.include_tool_reasoning();
    let max_iterations = 100;

    for iteration in 0..max_iterations {
        // Compact context if needed before building messages
        session.compact_if_needed().await;

        // Log iteration start
        if let Some(logger) = debug_logger {
            logger.log_iteration(iteration, "chat_completion");
        }

        let messages = session.build_messages();

        tracing::debug!(
            iteration = iteration,
            message_count = messages.len(),
            context_bytes = session.total_bytes(),
            "Chat LLM call context size"
        );

        // Log messages being sent
        if let Some(logger) = debug_logger {
            logger.log_messages_sent(&messages, model.as_deref());
        }

        let mut request = CompletionRequest::new(messages);

        if let Some(m) = model {
            request = request.with_model(m);
        }

        if let Some(temp) = cli.temperature {
            request = request.with_temperature(temp);
        }

        if let Some(max_tokens) = cli.max_tokens {
            request = request.with_max_tokens(max_tokens);
        }

        if !extra_params.is_empty() {
            request = request.with_extra(extra_params.clone());
        }

        request = request.with_tools(tools_registry.definitions());

        // Non-streaming mode: use complete() instead of stream()
        if cli.no_stream {
            let response = provider.complete(request).await?;

            tracing::debug!(
                content_len = response.message.content.to_string_lossy().len(),
                tool_calls = response.message.tool_calls.len(),
                "Non-streaming response received"
            );

            let content = response.message.content.to_string_lossy();
            let tool_calls = response.message.tool_calls.clone();

            // Log response received
            if let Some(logger) = debug_logger {
                logger.log_response_received(
                    content.len(),
                    None,
                    tool_calls.len(),
                    if tool_calls.is_empty() { "stop" } else { "tool_calls" },
                );
                logger.log_assistant_response(&content, None, tool_calls.len());
            }

            // Handle tool calls if any
            if !tool_calls.is_empty() {
                // Print any content before tool calls
                if !content.is_empty() {
                    print_section_header("Response")?;
                    let mut renderer = MarkdownRenderer::new();
                    renderer.push(&content)?;
                    renderer.finish()?;
                    println!();
                }

                // Add assistant message with tool calls; attach reasoning if configured
                let reasoning = if include_tool_reasoning {
                    response.thinking.clone()
                } else {
                    None
                };
                let assistant_msg =
                    Message::assistant_with_tool_calls(content.as_str(), tool_calls.clone())
                        .with_reasoning(reasoning);
                session.add_assistant_with_tools(assistant_msg);

                // Log message stored
                if let Some(logger) = debug_logger {
                    logger.log_message_stored("assistant", content.len(), true);
                }

                // Show tool calls to user and log them
                for tool_call in &tool_calls {
                    print_tool_call(&tool_call.name, &tool_call.arguments)?;
                    if let Some(logger) = debug_logger {
                        let args_preview = format_tool_args(&tool_call.arguments);
                        logger.log_tool_call(&tool_call.name, &args_preview);
                        logger.log_tool_call_full(&tool_call.id, &tool_call.name, &tool_call.arguments);
                    }
                }

                // Build tool_call_id → tool_name map for result logging
                let id_to_name: std::collections::HashMap<String, String> = tool_calls.iter()
                    .map(|tc| (tc.id.clone(), tc.name.clone()))
                    .collect();

                let results = execute_tools_parallel_with_chunker(
                    tools_registry,
                    tool_calls,
                    Some(chunk_processor),
                    Some(original_query),
                )
                .await;

                for result in results {
                    tracing::debug!(
                        tool_call_id = %result.tool_call_id,
                        result_len = result.content.len(),
                        is_error = result.is_error,
                        "Tool result received"
                    );

                    // Log tool result
                    if let Some(logger) = debug_logger {
                        logger.log_tool_result(&result.tool_call_id, result.content.len(), result.is_error);
                        let tool_name = id_to_name.get(&result.tool_call_id).map(|s| s.as_str()).unwrap_or("unknown");
                        logger.log_tool_result_full(&result.tool_call_id, tool_name, &result.content, result.is_error);
                    }

                    session.add_tool_result(&result.tool_call_id, &result.content);
                }

                // Continue to get next response
                continue;
            }

            // No tool calls - strip reasoning from history, print final response
            qq_core::message::strip_reasoning_from_history(&mut session.messages);
            print_section_header("Response")?;
            let mut renderer = MarkdownRenderer::new();
            renderer.push(&content)?;
            renderer.finish()?;
            session.add_assistant_message(&content);

            // Log final message stored
            if let Some(logger) = debug_logger {
                logger.log_message_stored("assistant", content.len(), false);
            }

            return Ok(());
        }

        // Streaming mode: use stream()
        let mut stream = provider.stream(request).await?;

        // Set up markdown renderers for thinking and content
        let mut thinking_renderer = MarkdownRenderer::new();
        let mut content_renderer = MarkdownRenderer::new();
        let mut in_thinking = false;
        let mut in_content = false;
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut current_tool_call: Option<(String, String, String)> = None; // (id, name, arguments)
        let mut accumulated_thinking = String::new();

        while let Some(chunk) = stream.next().await {
            match chunk? {
                StreamChunk::Start { .. } => {}
                StreamChunk::ThinkingDelta { content: delta } => {
                    if !in_thinking {
                        // Print thinking header
                        print_section_header("Thinking")?;
                        in_thinking = true;
                    }
                    if include_tool_reasoning {
                        accumulated_thinking.push_str(&delta);
                    }
                    thinking_renderer.push(&delta)?;
                }
                StreamChunk::Delta { content: delta } => {
                    if !in_content {
                        // Finish thinking section if we were in it
                        if in_thinking {
                            thinking_renderer.finish()?;
                        }
                        // Print content header
                        print_section_header("Response")?;
                        in_content = true;
                    }
                    content_renderer.push(&delta)?;
                }
                StreamChunk::ToolCallStart { id, name } => {
                    // Finish any pending tool call
                    if let Some((tc_id, tc_name, tc_args)) = current_tool_call.take() {
                        let args: serde_json::Value =
                            serde_json::from_str(&tc_args).unwrap_or(serde_json::Value::Null);
                        tool_calls.push(ToolCall::new(tc_id, tc_name, args));
                    }
                    current_tool_call = Some((id, name, String::new()));
                }
                StreamChunk::ToolCallDelta { arguments } => {
                    if let Some((_, _, ref mut args)) = current_tool_call {
                        args.push_str(&arguments);
                    }
                }
                StreamChunk::Done { usage } => {
                    // Finish any pending tool call
                    if let Some((tc_id, tc_name, tc_args)) = current_tool_call.take() {
                        let args: serde_json::Value =
                            serde_json::from_str(&tc_args).unwrap_or(serde_json::Value::Null);
                        tool_calls.push(ToolCall::new(tc_id, tc_name, args));
                    }

                    if let Some(u) = usage {
                        tracing::debug!(
                            prompt_tokens = u.prompt_tokens,
                            completion_tokens = u.completion_tokens,
                            iteration = iteration + 1,
                            "Stream iteration complete"
                        );
                    }
                }
                StreamChunk::Error { message } => {
                    return Err(anyhow::anyhow!("Stream error: {}", message));
                }
            }
        }

        // Get the actual content (not thinking) for storage
        let content = content_renderer.content().to_string();
        let thinking_len = if in_thinking {
            Some(thinking_renderer.content().len())
        } else {
            None
        };

        // Log response received
        if let Some(logger) = debug_logger {
            logger.log_response_received(
                content.len(),
                thinking_len,
                tool_calls.len(),
                if tool_calls.is_empty() { "stop" } else { "tool_calls" },
            );
            let thinking = if in_thinking { Some(thinking_renderer.content()) } else { None };
            logger.log_assistant_response(&content, thinking, tool_calls.len());
        }

        // Note: thinking_renderer content is displayed but NEVER stored in messages

        // Handle tool calls if any
        if !tool_calls.is_empty() {
            if !content_renderer.is_empty() {
                println!(); // Newline after any content
            }

            // Add assistant message with tool calls; attach reasoning if configured
            let reasoning = if include_tool_reasoning && !accumulated_thinking.is_empty() {
                Some(std::mem::take(&mut accumulated_thinking))
            } else {
                None
            };
            let assistant_msg =
                Message::assistant_with_tool_calls(content.as_str(), tool_calls.clone())
                    .with_reasoning(reasoning);
            session.add_assistant_with_tools(assistant_msg);

            // Log message stored
            if let Some(logger) = debug_logger {
                logger.log_message_stored("assistant", content.len(), true);
            }

            // Show tool calls to user and log them
            for tool_call in &tool_calls {
                print_tool_call(&tool_call.name, &tool_call.arguments)?;
                if let Some(logger) = debug_logger {
                    let args_preview = format_tool_args(&tool_call.arguments);
                    logger.log_tool_call(&tool_call.name, &args_preview);
                    logger.log_tool_call_full(&tool_call.id, &tool_call.name, &tool_call.arguments);
                }
            }

            // Build tool_call_id → tool_name map for result logging
            let id_to_name: std::collections::HashMap<String, String> = tool_calls.iter()
                .map(|tc| (tc.id.clone(), tc.name.clone()))
                .collect();

            let results = execute_tools_parallel_with_chunker(
                tools_registry,
                tool_calls,
                Some(chunk_processor),
                Some(original_query),
            )
            .await;

            for result in results {
                tracing::debug!(
                    tool_call_id = %result.tool_call_id,
                    result_len = result.content.len(),
                    is_error = result.is_error,
                    "Tool result received"
                );
                tracing::trace!(
                    tool_call_id = %result.tool_call_id,
                    content = %result.content,
                    "Tool result content"
                );

                // Log tool result
                if let Some(logger) = debug_logger {
                    logger.log_tool_result(&result.tool_call_id, result.content.len(), result.is_error);
                    let tool_name = id_to_name.get(&result.tool_call_id).map(|s| s.as_str()).unwrap_or("unknown");
                    logger.log_tool_result_full(&result.tool_call_id, tool_name, &result.content, result.is_error);
                }

                session.add_tool_result(&result.tool_call_id, &result.content);
            }

            // Continue to get next response
            continue;
        }

        // No tool calls - strip reasoning from history and finish up
        qq_core::message::strip_reasoning_from_history(&mut session.messages);
        if in_thinking && !in_content {
            // Only had thinking, no content
            thinking_renderer.finish()?;
        } else if in_content {
            content_renderer.finish()?;
        }
        session.add_assistant_message(&content);

        // Log final message stored
        if let Some(logger) = debug_logger {
            logger.log_message_stored("assistant", content.len(), false);
        }

        return Ok(());
    }

    // Log warning for max iterations
    if let Some(logger) = debug_logger {
        logger.log_warning(&format!("Max iterations ({}) reached", max_iterations));
    }
    eprintln!("Warning: Max iterations ({}) reached", max_iterations);
    Ok(())
}

fn get_history_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("qq").join("chat_history"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use qq_core::testing::MockCompactor;
    use qq_core::ObservationConfig;

    #[test]
    fn test_new_session_empty() {
        let session = ChatSession::new(Some("system".to_string()));
        assert_eq!(session.message_count(), 0);
        assert_eq!(session.total_bytes(), 0);
        assert!(session.system_prompt.is_some());
    }

    #[test]
    fn test_add_user_message() {
        let mut session = ChatSession::new(None);
        session.add_user_message("hello");
        assert_eq!(session.message_count(), 1);
        assert!(session.total_bytes() > 0);
    }

    #[test]
    fn test_add_assistant_message() {
        let mut session = ChatSession::new(None);
        session.add_assistant_message("response");
        assert_eq!(session.message_count(), 1);
    }

    #[test]
    fn test_add_tool_result() {
        let mut session = ChatSession::new(None);
        session.add_tool_result("tc-1", "tool output");
        assert_eq!(session.message_count(), 1);
    }

    #[test]
    fn test_message_count_and_total_bytes() {
        let mut session = ChatSession::new(None);
        session.add_user_message("hello");
        session.add_assistant_message("world");
        assert_eq!(session.message_count(), 2);
        assert_eq!(session.total_bytes(), 10); // "hello" + "world"
    }

    #[test]
    fn test_build_messages_with_system_prompt() {
        let mut session = ChatSession::new(Some("Be helpful.".to_string()));
        session.add_user_message("hi");

        let msgs = session.build_messages();
        assert_eq!(msgs.len(), 2); // system + user
        assert_eq!(msgs[0].content.as_text(), Some("Be helpful."));
    }

    #[test]
    fn test_build_messages_without_system_prompt() {
        let mut session = ChatSession::new(None);
        session.add_user_message("hi");

        let msgs = session.build_messages();
        assert_eq!(msgs.len(), 1); // just user
    }

    #[test]
    fn test_build_messages_with_observation_log() {
        let config = ObservationConfig {
            message_threshold_bytes: 10,
            preserve_recent: 1,
            hysteresis: 1.0,
            ..Default::default()
        };
        let mut session = ChatSession::new(Some("system".to_string()))
            .with_observation_config(config);

        // When observation_log is empty, no extra system message should be added.
        session.add_user_message("hi");
        let msgs = session.build_messages();
        assert_eq!(msgs.len(), 2); // system + user (no observation log)
    }

    #[test]
    fn test_build_messages_empty_observation_log() {
        let mut session = ChatSession::new(Some("system".to_string()));
        session.add_user_message("hi");
        session.add_assistant_message("hello");

        let msgs = session.build_messages();
        // Should be: system + user + assistant (no observation log message)
        assert_eq!(msgs.len(), 3);
    }

    #[test]
    fn test_clear_resets_messages_and_observation_memory() {
        let mut session = ChatSession::new(None);
        session.add_user_message("hello");
        session.add_assistant_message("world");

        session.clear();

        assert_eq!(session.message_count(), 0);
        assert_eq!(session.total_bytes(), 0);
        assert_eq!(session.observation_memory.observation_count, 0);
        assert_eq!(session.observation_memory.reflection_count, 0);
    }

    #[tokio::test]
    async fn test_compact_if_needed_triggers_observation() {
        let config = ObservationConfig {
            message_threshold_bytes: 200,
            observation_threshold_bytes: 100_000,
            preserve_recent: 2,
            hysteresis: 1.0,
            ..Default::default()
        };

        let compactor = Arc::new(MockCompactor::new());
        compactor.queue_observe(Ok("## Observations\n- key finding".to_string()));

        let mut session = ChatSession::new(None)
            .with_observation_config(config)
            .with_compactor(compactor.clone());

        // Add enough messages to exceed threshold
        for _ in 0..8 {
            session.add_user_message(&"x".repeat(100));
        }
        let original_count = session.message_count();

        session.compact_if_needed().await;

        // Messages should have been compacted
        assert!(session.message_count() < original_count);
        assert_eq!(session.observation_memory.observation_count, 1);
        assert!(session.observation_memory.observation_log().contains("key finding"));
    }

    #[tokio::test]
    async fn test_compact_if_needed_no_compactor_does_nothing() {
        let mut session = ChatSession::new(None);
        for _ in 0..20 {
            session.add_user_message(&"x".repeat(100));
        }
        let count = session.message_count();

        session.compact_if_needed().await;

        assert_eq!(session.message_count(), count); // No change
    }

    #[tokio::test]
    async fn test_compact_if_needed_below_threshold_does_nothing() {
        let config = ObservationConfig {
            message_threshold_bytes: 50_000,
            preserve_recent: 10,
            hysteresis: 1.0,
            ..Default::default()
        };

        let compactor = Arc::new(MockCompactor::new());
        compactor.queue_observe(Ok("should not be called".to_string()));

        let mut session = ChatSession::new(None)
            .with_observation_config(config)
            .with_compactor(compactor);

        session.add_user_message("short");
        session.add_assistant_message("also short");

        session.compact_if_needed().await;

        assert_eq!(session.message_count(), 2); // No compaction
        assert_eq!(session.observation_memory.observation_count, 0);
    }

    #[tokio::test]
    async fn test_full_lifecycle() {
        let config = ObservationConfig {
            message_threshold_bytes: 200,
            observation_threshold_bytes: 100_000,
            preserve_recent: 2,
            hysteresis: 1.0,
            ..Default::default()
        };

        let compactor = Arc::new(MockCompactor::new());
        compactor.queue_observe(Ok("## Round 1\n- First observations".to_string()));
        compactor.queue_observe(Ok("## Round 2\n- More observations".to_string()));

        let mut session = ChatSession::new(Some("test system".to_string()))
            .with_observation_config(config)
            .with_compactor(compactor);

        // Add messages and compact
        for _ in 0..8 {
            session.add_user_message(&"x".repeat(100));
        }
        session.compact_if_needed().await;
        assert_eq!(session.observation_memory.observation_count, 1);

        // Build messages should include observation log
        let msgs = session.build_messages();
        let has_obs_log = msgs.iter().any(|m| {
            m.content.to_string_lossy().contains("Observation Log")
        });
        assert!(has_obs_log);

        // Add more messages and compact again
        for _ in 0..8 {
            session.add_user_message(&"y".repeat(100));
        }
        session.compact_if_needed().await;
        assert_eq!(session.observation_memory.observation_count, 2);

        // Observation log should contain both rounds
        let log = session.observation_memory.observation_log();
        assert!(log.contains("First observations"));
        assert!(log.contains("More observations"));
    }
}
