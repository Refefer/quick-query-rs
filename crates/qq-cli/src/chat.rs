//! Interactive chat mode with readline support.

use std::path::PathBuf;

use anyhow::Result;
use rustyline::error::ReadlineError;
use rustyline::history::FileHistory;
use rustyline::{Config, Editor};

use futures::StreamExt;

use qq_core::{
    execute_tools_parallel, CompletionRequest, Message, Provider, StreamChunk, ToolCall,
    ToolRegistry,
};

use crate::config::Config as AppConfig;
use crate::markdown::MarkdownRenderer;
use crate::Cli;

/// Chat session state
pub struct ChatSession {
    messages: Vec<Message>,
    system_prompt: Option<String>,
}

impl ChatSession {
    pub fn new(system_prompt: Option<String>) -> Self {
        Self {
            messages: Vec::new(),
            system_prompt,
        }
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
}

/// Chat commands
enum ChatCommand {
    Quit,
    Clear,
    History,
    Help,
    Tools,
    System(String),
    None(String), // Regular message
}

fn parse_command(input: &str) -> ChatCommand {
    let trimmed = input.trim();

    if trimmed.is_empty() {
        return ChatCommand::None(String::new());
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
        "/history" | "/h" => ChatCommand::History,
        "/help" | "/?" => ChatCommand::Help,
        "/tools" | "/t" => ChatCommand::Tools,
        "/system" | "/sys" => ChatCommand::System(arg),
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
  /help, /?      Show this help message
  /quit, /exit   Exit chat mode
  /clear, /c     Clear conversation history
  /history, /h   Show message count
  /tools, /t     List available tools
  /system <msg>  Set a new system prompt

Tips:
  - Press Ctrl+C to cancel current generation
  - Press Ctrl+D to exit
  - Up/Down arrows navigate history
"#
    );
}

/// Run interactive chat mode
pub async fn run_chat(
    cli: &Cli,
    _config: &AppConfig,
    provider: Box<dyn Provider>,
    system_prompt: Option<String>,
    tools_registry: ToolRegistry,
    extra_params: std::collections::HashMap<String, serde_json::Value>,
    model: Option<String>,
) -> Result<()> {
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

    let mut session = ChatSession::new(system_prompt);

    println!("Chat mode started. Type /help for commands, /quit to exit.\n");

    loop {
        let prompt = format!("you> ");

        match rl.readline(&prompt) {
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
                    ChatCommand::Tools => {
                        println!("\nAvailable tools:");
                        for def in tools_registry.definitions() {
                            println!("  {} - {}", def.name, def.description);
                        }
                        println!();
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
                    ChatCommand::None(text) => {
                        if text.is_empty() {
                            continue;
                        }

                        session.add_user_message(&text);

                        // Run completion loop
                        match run_completion(
                            cli,
                            &provider,
                            &mut session,
                            &tools_registry,
                            &extra_params,
                            &model,
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
    provider: &Box<dyn Provider>,
    session: &mut ChatSession,
    tools_registry: &ToolRegistry,
    extra_params: &std::collections::HashMap<String, serde_json::Value>,
    model: &Option<String>,
) -> Result<()> {
    let max_iterations = 20;

    for iteration in 0..max_iterations {
        let mut request = CompletionRequest::new(session.build_messages());

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

        // Stream the response
        let mut stream = provider.stream(request).await?;

        // Set up markdown renderer for streaming output
        let mut renderer = MarkdownRenderer::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut current_tool_call: Option<(String, String, String)> = None; // (id, name, arguments)

        while let Some(chunk) = stream.next().await {
            match chunk? {
                StreamChunk::Start { .. } => {}
                StreamChunk::Delta { content: delta } => {
                    if renderer.is_empty() && !delta.trim().is_empty() {
                        renderer.print_prefix()?;
                    }
                    renderer.push(&delta)?;
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

                    if cli.debug {
                        if let Some(u) = usage {
                            eprintln!(
                                "\n[tokens: {} prompt, {} completion | iterations: {}]",
                                u.prompt_tokens,
                                u.completion_tokens,
                                iteration + 1
                            );
                        }
                    }
                }
                StreamChunk::Error { message } => {
                    return Err(anyhow::anyhow!("Stream error: {}", message));
                }
            }
        }

        let content = renderer.content().to_string();

        // Handle tool calls if any
        if !tool_calls.is_empty() {
            if !renderer.is_empty() {
                println!(); // Newline after any content
            }

            // Add assistant message with tool calls to session
            let assistant_msg =
                Message::assistant_with_tool_calls(content.as_str(), tool_calls.clone());
            session.add_assistant_with_tools(assistant_msg);

            if cli.debug {
                for tool_call in &tool_calls {
                    eprintln!("[tool] {}({})", tool_call.name, tool_call.arguments);
                }
            }

            let results = execute_tools_parallel(tools_registry, tool_calls).await;

            for result in results {
                if cli.debug {
                    let preview = if result.content.len() > 100 {
                        format!("{}...", &result.content[..100])
                    } else {
                        result.content.clone()
                    };
                    eprintln!("[result] {}", preview);
                }

                session.add_tool_result(&result.tool_call_id, &result.content);
            }

            // Continue to get next response
            continue;
        }

        // No tool calls - finish up
        renderer.finish()?;
        session.add_assistant_message(&content);

        return Ok(());
    }

    eprintln!("Warning: Max iterations ({}) reached", max_iterations);
    Ok(())
}

fn get_history_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("qq").join("chat_history"))
}
