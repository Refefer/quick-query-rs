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
    CompletionRequest, Message, Provider, Role, StreamChunk, ToolCall, ToolRegistry,
};

use crate::agents::AgentExecutor;
use crate::config::Config as AppConfig;
use crate::debug_log::DebugLogger;
use crate::event_bus::{AgentEvent, AgentEventBus};
use crate::markdown::MarkdownRenderer;
use crate::Cli;

/// Chat session state
pub struct ChatSession {
    pub messages: Vec<Message>,
    pub system_prompt: Option<String>,
    provider: Option<Arc<dyn Provider>>,
    /// Maximum context size in bytes before compaction triggers (default: 500KB)
    max_context_bytes: usize,
    /// Number of recent messages to always preserve during compaction (default: 10)
    preserve_recent: usize,
    /// Whether compaction has occurred during this session
    pub compaction_count: u32,
    /// Prompt used for LLM-powered context compaction
    compact_prompt: String,
}

/// Default max context bytes (500KB)
const DEFAULT_MAX_CONTEXT_BYTES: usize = 500_000;
/// Default number of recent messages to preserve
const DEFAULT_PRESERVE_RECENT: usize = 10;
/// Hysteresis margin: don't compact unless we're at least 10% over the limit
const COMPACTION_HYSTERESIS: f64 = 1.1;

/// Prompt for generating a conversation summary for context compaction.
const COMPACTION_SUMMARY_PROMPT: &str = r#"The conversation history is getting long and needs to be compacted. Summarize the conversation so far in a way that preserves:

1. Key decisions and conclusions reached
2. Important facts, file paths, code snippets, or data mentioned
3. The user's goals and preferences expressed
4. Any pending tasks or unresolved questions

Write a concise but comprehensive summary. Focus on information that would be needed to continue the conversation coherently. Do not include greetings or pleasantries."#;

impl ChatSession {
    pub fn new(system_prompt: Option<String>) -> Self {
        Self {
            messages: Vec::new(),
            system_prompt,
            provider: None,
            max_context_bytes: DEFAULT_MAX_CONTEXT_BYTES,
            preserve_recent: DEFAULT_PRESERVE_RECENT,
            compaction_count: 0,
            compact_prompt: COMPACTION_SUMMARY_PROMPT.to_string(),
        }
    }

    /// Create a new session with a provider for LLM-powered compaction.
    pub fn with_provider(mut self, provider: Arc<dyn Provider>) -> Self {
        self.provider = Some(provider);
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

        if let Some(system) = &self.system_prompt {
            msgs.push(Message::system(system.as_str()));
        }

        msgs.extend(self.messages.clone());
        msgs
    }

    pub fn clear(&mut self) {
        self.messages.clear();
    }

    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Calculate total byte count of all messages in the session.
    pub fn total_bytes(&self) -> usize {
        self.messages.iter().map(|m| m.byte_count()).sum()
    }

    /// Compact the conversation history if it exceeds the context limit.
    ///
    /// Uses a tiered strategy:
    /// - Tier 1: LLM summarization of older messages (preferred)
    /// - Tier 2: Partial truncation + summarization (fallback)
    /// - Tier 3: Simple truncation (last resort)
    pub async fn compact_if_needed(&mut self) {
        let total = self.total_bytes();
        let threshold = (self.max_context_bytes as f64 * COMPACTION_HYSTERESIS) as usize;

        if total <= threshold {
            return;
        }

        // Don't compact if we have very few messages
        if self.messages.len() <= self.preserve_recent + 2 {
            return;
        }

        let preserve_count = self.preserve_recent.min(self.messages.len());
        let compact_end = self.messages.len() - preserve_count;

        // Ensure we don't split in the middle of a tool call sequence.
        // Walk backwards from compact_end to find a safe split point.
        let safe_end = find_safe_split_point(&self.messages, compact_end);
        if safe_end <= 1 {
            // Nothing meaningful to compact
            return;
        }

        let old_messages = &self.messages[..safe_end];

        tracing::info!(
            total_bytes = total,
            max_bytes = self.max_context_bytes,
            messages_to_compact = safe_end,
            messages_to_preserve = self.messages.len() - safe_end,
            "Context compaction triggered"
        );

        // Tier 1: Try LLM summarization
        if let Some(ref provider) = self.provider {
            match self.try_llm_summary(provider.clone(), old_messages).await {
                Ok(summary) => {
                    self.apply_compaction(safe_end, &summary);
                    return;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Tier 1 LLM summarization failed, trying tier 2");
                }
            }

            // Tier 2: Try summarizing just the first half of old messages
            let half = safe_end / 2;
            if half > 0 {
                let first_half = &self.messages[..half];
                match self.try_llm_summary(provider.clone(), first_half).await {
                    Ok(summary) => {
                        self.apply_compaction(half, &summary);
                        return;
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "Tier 2 partial summarization failed, falling back to truncation"
                        );
                    }
                }
            }
        }

        // Tier 3: Simple truncation (no provider or all LLM calls failed)
        tracing::warn!(
            messages_dropped = safe_end,
            "Tier 3 truncation: dropping old messages without summary"
        );
        let summary = format!(
            "[Earlier conversation ({} messages, {} bytes) was truncated to save memory. \
            Recent context is preserved below.]",
            safe_end,
            old_messages.iter().map(|m| m.byte_count()).sum::<usize>()
        );
        self.apply_compaction(safe_end, &summary);
    }

    /// Attempt LLM-powered summarization of messages.
    async fn try_llm_summary(
        &self,
        provider: Arc<dyn Provider>,
        messages_to_summarize: &[Message],
    ) -> Result<String> {
        let mut summary_messages: Vec<Message> = messages_to_summarize.to_vec();
        summary_messages.push(Message::user(self.compact_prompt.as_str()));

        let request = CompletionRequest::new(summary_messages);
        let response = provider.complete(request).await?;

        let summary = response.message.content.to_string_lossy();
        if summary.is_empty() {
            return Err(anyhow::anyhow!("LLM returned empty summary"));
        }

        Ok(summary)
    }

    /// Apply compaction: replace old messages with a summary, keep recent messages.
    fn apply_compaction(&mut self, compact_up_to: usize, summary: &str) {
        let old_count = self.messages.len();
        let old_bytes = self.total_bytes();

        // Keep recent messages
        let recent: Vec<Message> = self.messages.drain(compact_up_to..).collect();

        // Replace all old messages with a single summary message
        self.messages.clear();
        let summary_text = format!("## Conversation Summary\n\n{}", summary);
        self.messages.push(Message::system(summary_text.as_str()));
        self.messages.extend(recent);
        self.messages.shrink_to_fit();

        self.compaction_count += 1;

        let new_bytes = self.total_bytes();
        tracing::info!(
            old_messages = old_count,
            new_messages = self.messages.len(),
            old_bytes = old_bytes,
            new_bytes = new_bytes,
            bytes_freed = old_bytes.saturating_sub(new_bytes),
            compaction_number = self.compaction_count,
            "Context compaction complete"
        );
    }
}

/// Find a safe point to split messages that doesn't break tool call sequences.
///
/// A tool call sequence is: assistant message with tool_calls followed by
/// one or more tool result messages. We should never split in the middle.
fn find_safe_split_point(messages: &[Message], desired_end: usize) -> usize {
    let mut end = desired_end;

    // Walk backwards to find a position that's not inside a tool call sequence
    while end > 0 {
        let msg = &messages[end - 1];

        // If the message at end-1 is a tool result, we're inside a sequence.
        // Keep walking back until we find the assistant message that started it.
        if msg.tool_call_id.is_some() {
            end -= 1;
            continue;
        }

        // If the message at end-1 is an assistant with tool_calls,
        // the results come after it, so we'd split right before the results.
        // That's bad - include this message's results OR exclude the assistant msg.
        if msg.role == Role::Assistant && !msg.tool_calls.is_empty() {
            // Exclude this assistant message too, so the tool results stay with it
            end -= 1;
            continue;
        }

        // Safe position found
        break;
    }

    end
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
    if trimmed.starts_with('@') {
        let parts: Vec<&str> = trimmed[1..].splitn(2, char::is_whitespace).collect();
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
  /clear, /c          Clear conversation history
  /reset              Clear conversation + all agent memory
  /history, /h        Show message count
  /memory, /mem       Show memory usage diagnostics
  /tools, /t          List available tools
  /agents, /a         List available agents
  /delegate <a> <t>   Delegate task <t> to agent <a>
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
        .with_provider(Arc::clone(&provider));

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
                        println!("Session reset (conversation + agent memory cleared).\n");
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
                        println!("  Messages:       {}", session.message_count());
                        println!("  Context bytes:  {}", format_bytes(session.total_bytes()));
                        println!("  Max context:    {}", format_bytes(DEFAULT_MAX_CONTEXT_BYTES));
                        println!("  Compactions:    {}", session.compaction_count);
                        if let Some(rss) = get_rss_bytes() {
                            println!("  Process RSS:    {}", format_bytes(rss));
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

                // Add assistant message with tool calls
                let assistant_msg =
                    Message::assistant_with_tool_calls(content.as_str(), tool_calls.clone());
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

            // No tool calls - print final response
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

        while let Some(chunk) = stream.next().await {
            match chunk? {
                StreamChunk::Start { .. } => {}
                StreamChunk::ThinkingDelta { content: delta } => {
                    if !in_thinking {
                        // Print thinking header
                        print_section_header("Thinking")?;
                        in_thinking = true;
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

            // Add assistant message with tool calls - only store actual content, not thinking
            let assistant_msg =
                Message::assistant_with_tool_calls(content.as_str(), tool_calls.clone());
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

        // No tool calls - finish up
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
