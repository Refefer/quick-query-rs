//! Interactive runner for agent execution.
//!
//! The InteractiveRunner orchestrates interactive agent execution, coordinating
//! between the agent (LLM + tools) and the user interface.

use std::sync::Arc;

use anyhow::Result;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use tokio::sync::RwLock;

use qq_core::{
    ChunkProcessor, ChunkerConfig, CompletionRequest, Message, Provider, StreamChunk, ToolCall,
    ToolExecutionResult, ToolRegistry,
};

use crate::agents::{AgentExecutor, InternalAgent};
use crate::chat::ChatSession;
use crate::event_bus::AgentEventBus;
use crate::execution_context::ExecutionContext;
use crate::interface::{AgentInterface, AgentOutput, InterfaceCommand, UserInput};

/// Configuration for the InteractiveRunner.
pub struct RunnerConfig {
    /// Model to use for completions.
    pub model: Option<String>,

    /// Temperature for generation.
    pub temperature: Option<f32>,

    /// Maximum tokens to generate.
    pub max_tokens: Option<u32>,

    /// Extra parameters to pass to the API.
    pub extra_params: std::collections::HashMap<String, serde_json::Value>,

    /// Chunker configuration for large outputs.
    pub chunker_config: ChunkerConfig,

    /// Maximum iterations for tool loops.
    pub max_iterations: u32,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        Self {
            model: None,
            temperature: None,
            max_tokens: None,
            extra_params: std::collections::HashMap::new(),
            chunker_config: ChunkerConfig::default(),
            max_iterations: 100,
        }
    }
}

/// Interactive runner that orchestrates agent execution with a user interface.
pub struct InteractiveRunner<I: AgentInterface> {
    /// The user interface.
    interface: I,

    /// The primary agent configuration.
    agent: Box<dyn InternalAgent>,

    /// Chat session for conversation history.
    session: ChatSession,

    /// Provider for LLM calls.
    provider: Arc<dyn Provider>,

    /// Tools registry.
    tools: Arc<ToolRegistry>,

    /// Agent executor for manual delegation.
    agent_executor: Option<Arc<RwLock<AgentExecutor>>>,

    /// Execution context for tracking call stack.
    execution_context: ExecutionContext,

    /// Event bus for agent progress (optional).
    event_bus: Option<AgentEventBus>,

    /// Runner configuration.
    config: RunnerConfig,
}

impl<I: AgentInterface> InteractiveRunner<I> {
    /// Create a new InteractiveRunner.
    pub fn new(
        interface: I,
        agent: Box<dyn InternalAgent>,
        system_prompt: Option<String>,
        provider: Arc<dyn Provider>,
        tools: Arc<ToolRegistry>,
        config: RunnerConfig,
    ) -> Self {
        // Use provided system prompt or agent's default
        let effective_prompt = system_prompt.or_else(|| Some(agent.system_prompt().to_string()));
        let session = ChatSession::new(effective_prompt);

        Self {
            interface,
            agent,
            session,
            provider,
            tools,
            agent_executor: None,
            execution_context: ExecutionContext::new(),
            event_bus: None,
            config,
        }
    }

    /// Set the agent executor for manual delegation.
    pub fn with_agent_executor(mut self, executor: Arc<RwLock<AgentExecutor>>) -> Self {
        self.agent_executor = Some(executor);
        self
    }

    /// Set the execution context.
    pub fn with_execution_context(mut self, ctx: ExecutionContext) -> Self {
        self.execution_context = ctx;
        self
    }

    /// Set the event bus.
    pub fn with_event_bus(mut self, bus: AgentEventBus) -> Self {
        self.event_bus = Some(bus);
        self
    }

    /// Run the interactive loop.
    pub async fn run(&mut self) -> Result<()> {
        // Initialize the interface
        self.interface.initialize().await?;

        // Main loop
        loop {
            // Get user input
            let input = match self.interface.next_input().await? {
                Some(input) => input,
                None => break, // User quit
            };

            // Process the input
            match input {
                UserInput::Message(text) => {
                    self.process_message(&text).await?;
                }
                UserInput::Command(cmd) => {
                    if !self.handle_command(cmd).await? {
                        break; // Quit requested
                    }
                }
                UserInput::Cancel => {
                    // Cancel current operation - nothing running in blocking mode
                }
                UserInput::Empty => {
                    // Skip empty input
                    continue;
                }
            }

            if self.interface.should_quit() {
                break;
            }
        }

        // Cleanup the interface
        self.interface.cleanup().await?;

        Ok(())
    }

    /// Process a user message through the agent.
    async fn process_message(&mut self, text: &str) -> Result<()> {
        // Add user message to session
        self.session.add_user_message(text);

        // Signal response starting
        self.interface.start_response(text);
        self.interface.set_streaming(true);

        // Create chunk processor for large outputs
        let chunk_processor = ChunkProcessor::new(
            Arc::clone(&self.provider),
            self.config.chunker_config.clone(),
        );

        // Run the completion loop
        let result = self
            .run_completion_loop(text, &chunk_processor)
            .await;

        // Handle result
        match result {
            Ok(content) => {
                // Add assistant response to session
                if !content.is_empty() {
                    self.session.add_assistant_message(&content);
                }
                self.interface
                    .emit(AgentOutput::Done {
                        content,
                        usage: None,
                    })
                    .await?;
            }
            Err(e) => {
                self.interface
                    .emit(AgentOutput::Error {
                        message: e.to_string(),
                    })
                    .await?;
                // Remove the failed user message
                self.session.messages.pop();
            }
        }

        self.interface.set_streaming(false);
        self.interface.finish_response();

        Ok(())
    }

    /// Run the completion loop with tool execution.
    async fn run_completion_loop(
        &mut self,
        original_query: &str,
        chunk_processor: &ChunkProcessor,
    ) -> Result<String> {
        let mut messages = self.session.build_messages();

        for iteration in 0..self.config.max_iterations {
            self.interface
                .emit(AgentOutput::IterationStart { iteration: iteration + 1 })
                .await?;

            // Calculate input bytes
            let input_bytes = serde_json::to_string(&messages)
                .map(|s| s.len())
                .unwrap_or(0);

            // Build request
            let mut request = CompletionRequest::new(messages.clone());

            if let Some(ref m) = self.config.model {
                request = request.with_model(m);
            }

            if let Some(temp) = self.config.temperature {
                request = request.with_temperature(temp);
            }

            if let Some(max_tok) = self.config.max_tokens {
                request = request.with_max_tokens(max_tok);
            }

            if !self.config.extra_params.is_empty() {
                request = request.with_extra(self.config.extra_params.clone());
            }

            request = request.with_tools(self.tools.definitions());

            // Stream the response
            let mut stream = self.provider.stream(request).await?;

            let mut content = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut current_tool_call: Option<(String, String, String)> = None;
            let mut output_bytes: usize = 0;

            while let Some(chunk) = stream.next().await {
                match chunk? {
                    StreamChunk::Start { model } => {
                        self.interface
                            .emit(AgentOutput::StreamStart { model })
                            .await?;
                    }
                    StreamChunk::ThinkingDelta { content: delta } => {
                        output_bytes += delta.len();
                        self.interface
                            .emit(AgentOutput::ThinkingDelta(delta))
                            .await?;
                    }
                    StreamChunk::Delta { content: delta } => {
                        output_bytes += delta.len();
                        content.push_str(&delta);
                        self.interface
                            .emit(AgentOutput::ContentDelta(delta))
                            .await?;
                    }
                    StreamChunk::ToolCallStart { id, name } => {
                        // Finish pending tool call
                        if let Some((tc_id, tc_name, tc_args)) = current_tool_call.take() {
                            let args: serde_json::Value =
                                serde_json::from_str(&tc_args).unwrap_or(serde_json::Value::Null);
                            tool_calls.push(ToolCall::new(tc_id, tc_name, args));
                        }
                        current_tool_call = Some((id.clone(), name.clone(), String::new()));
                        self.interface
                            .emit(AgentOutput::ToolStarted {
                                id,
                                name,
                            })
                            .await?;
                    }
                    StreamChunk::ToolCallDelta { arguments } => {
                        output_bytes += arguments.len();
                        if let Some((_, _, ref mut args)) = current_tool_call {
                            args.push_str(&arguments);
                        }
                    }
                    StreamChunk::Done { usage } => {
                        // Finish pending tool call
                        if let Some((tc_id, tc_name, tc_args)) = current_tool_call.take() {
                            let args: serde_json::Value =
                                serde_json::from_str(&tc_args).unwrap_or(serde_json::Value::Null);
                            tool_calls.push(ToolCall::new(tc_id, tc_name, args));
                        }

                        // Send byte counts
                        self.interface
                            .emit(AgentOutput::ByteCount {
                                input_bytes,
                                output_bytes,
                            })
                            .await?;

                        if tool_calls.is_empty() {
                            // No tool calls - we're done
                            self.execution_context.reset().await;
                            return Ok(content);
                        }

                        // Update usage if available
                        if let Some(u) = usage {
                            self.interface
                                .emit(AgentOutput::Done {
                                    content: String::new(),
                                    usage: Some(u),
                                })
                                .await?;
                        }
                    }
                    StreamChunk::Error { message } => {
                        self.execution_context.reset().await;
                        return Err(anyhow::anyhow!("Stream error: {}", message));
                    }
                }
            }

            // Handle tool calls
            if !tool_calls.is_empty() {
                // Add assistant message with tool calls
                let assistant_msg =
                    Message::assistant_with_tool_calls(content.as_str(), tool_calls.clone());
                messages.push(assistant_msg.clone());
                self.session.add_assistant_with_tools(assistant_msg);

                // Execute tools
                let results = self
                    .execute_tools(&tool_calls, chunk_processor, original_query)
                    .await;

                for result in results {
                    messages.push(Message::tool_result(&result.tool_call_id, &result.content));
                    self.session
                        .add_tool_result(&result.tool_call_id, &result.content);
                }

                // Clear content for next iteration
                content.clear();
                continue;
            }

            // No tool calls, we're done
            self.execution_context.reset().await;
            return Ok(content);
        }

        // Max iterations reached
        self.execution_context.reset().await;
        Err(anyhow::anyhow!(
            "Max iterations ({}) reached",
            self.config.max_iterations
        ))
    }

    /// Execute tool calls in parallel.
    async fn execute_tools(
        &mut self,
        tool_calls: &[ToolCall],
        chunk_processor: &ChunkProcessor,
        original_query: &str,
    ) -> Vec<ToolExecutionResult> {
        // Signal tools executing
        for tool_call in tool_calls {
            self.execution_context.push_tool(&tool_call.name).await;
            let _ = self
                .interface
                .emit(AgentOutput::ToolExecuting {
                    name: tool_call.name.clone(),
                })
                .await;
        }

        // Create futures for parallel execution
        let mut futures: FuturesUnordered<_> = tool_calls
            .iter()
            .map(|tool_call| {
                let registry = Arc::clone(&self.tools);
                let tool_call_id = tool_call.id.clone();
                let tool_name = tool_call.name.clone();
                let arguments = tool_call.arguments.clone();

                async move {
                    let result = if let Some(tool) = registry.get(&tool_name) {
                        match tool.execute(arguments).await {
                            Ok(output) => ToolExecutionResult {
                                tool_call_id: tool_call_id.clone(),
                                content: if output.is_error {
                                    format!("Error: {}", output.content)
                                } else {
                                    output.content
                                },
                                is_error: output.is_error,
                            },
                            Err(e) => ToolExecutionResult {
                                tool_call_id: tool_call_id.clone(),
                                content: format!("Error executing tool: {}", e),
                                is_error: true,
                            },
                        }
                    } else {
                        ToolExecutionResult {
                            tool_call_id: tool_call_id.clone(),
                            content: format!("Error: Unknown tool '{}'", tool_name),
                            is_error: true,
                        }
                    };
                    (tool_name, result)
                }
            })
            .collect();

        // Collect results as they complete
        let mut results = Vec::new();
        while let Some((tool_name, mut result)) = futures.next().await {
            // Apply chunking if needed
            if !result.is_error && chunk_processor.should_chunk(&result.content) {
                if let Ok(processed) = chunk_processor
                    .process_large_content(&result.content, Some(original_query))
                    .await
                {
                    result.content = processed;
                }
            }

            // Pop tool context
            self.execution_context.pop().await;

            // Emit completion event
            let _ = self
                .interface
                .emit(AgentOutput::ToolCompleted {
                    id: result.tool_call_id.clone(),
                    name: tool_name,
                    result_len: result.content.len(),
                    is_error: result.is_error,
                })
                .await;

            results.push(result);
        }

        results
    }

    /// Handle a command from the user.
    ///
    /// Returns false if the runner should quit.
    async fn handle_command(&mut self, cmd: InterfaceCommand) -> Result<bool> {
        match cmd {
            InterfaceCommand::Quit => {
                return Ok(false);
            }
            InterfaceCommand::ClearHistory => {
                self.session.clear();
                self.interface
                    .emit(AgentOutput::Status("Conversation cleared.".to_string()))
                    .await?;
            }
            InterfaceCommand::Reset => {
                self.session.clear();
                self.interface
                    .emit(AgentOutput::Status("Session reset.".to_string()))
                    .await?;
            }
            InterfaceCommand::ListTools => {
                let tools: Vec<String> = self
                    .tools
                    .definitions()
                    .iter()
                    .map(|d| format!("  {} - {}", d.name, d.description))
                    .collect();
                let msg = format!("Available tools:\n{}", tools.join("\n"));
                self.interface.emit(AgentOutput::Status(msg)).await?;
            }
            InterfaceCommand::ListAgents => {
                if let Some(ref executor) = self.agent_executor {
                    let exec = executor.read().await;
                    let agents = exec.list_agents();
                    if agents.is_empty() {
                        self.interface
                            .emit(AgentOutput::Status("No agents available.".to_string()))
                            .await?;
                    } else {
                        let list: Vec<String> = agents
                            .iter()
                            .map(|a| {
                                let marker = if a.is_internal {
                                    "(built-in)"
                                } else {
                                    "(external)"
                                };
                                format!("  Agent[{}] {} - {}", a.name, marker, a.description)
                            })
                            .collect();
                        let msg = format!("Available agents:\n{}", list.join("\n"));
                        self.interface.emit(AgentOutput::Status(msg)).await?;
                    }
                } else {
                    self.interface
                        .emit(AgentOutput::Status("Agents not configured.".to_string()))
                        .await?;
                }
            }
            InterfaceCommand::Help => {
                let help = r#"Commands:
  /help, /?           Show this help message
  /quit, /exit, /q    Exit
  /clear, /c          Clear conversation history
  /reset              Reset session (clear history and stats)
  /history, /h        Show message count
  /tools, /t          List available tools
  /agents, /a         List available agents
  /delegate <a> <t>   Delegate task <t> to agent <a>
  /system [msg]       Show or set system prompt

Agents:
  @agent <task>       Quick agent invocation (e.g., @explore Find all tests)

Navigation:
  Ctrl+C              Cancel current generation
  Ctrl+D              Exit"#;
                self.interface
                    .emit(AgentOutput::Status(help.to_string()))
                    .await?;
            }
            InterfaceCommand::History => {
                let count = self.session.message_count();
                self.interface
                    .emit(AgentOutput::Status(format!(
                        "Messages in conversation: {} ({} turns)",
                        count,
                        count / 2
                    )))
                    .await?;
            }
            InterfaceCommand::Delegate { agent, task } => {
                if let Some(ref executor) = self.agent_executor {
                    let exec = executor.read().await;
                    if !exec.has_agent(&agent) {
                        self.interface
                            .emit(AgentOutput::Error {
                                message: format!(
                                    "Unknown or disabled agent: {}. Use /agents to list available.",
                                    agent
                                ),
                            })
                            .await?;
                        return Ok(true);
                    }

                    self.interface
                        .emit(AgentOutput::Status(format!("Delegating to agent: {}", agent)))
                        .await?;
                    self.interface.set_streaming(true);

                    match exec.run(&agent, &task).await {
                        Ok(response) => {
                            self.interface
                                .emit(AgentOutput::ContentDelta(response))
                                .await?;
                        }
                        Err(e) => {
                            self.interface
                                .emit(AgentOutput::Error {
                                    message: format!("Agent error: {}", e),
                                })
                                .await?;
                        }
                    }

                    self.interface.set_streaming(false);
                } else {
                    self.interface
                        .emit(AgentOutput::Error {
                            message: "Agents not configured.".to_string(),
                        })
                        .await?;
                }
            }
            InterfaceCommand::System(new_prompt) => {
                if let Some(prompt) = new_prompt {
                    self.session.system_prompt = Some(prompt);
                    self.interface
                        .emit(AgentOutput::Status("System prompt updated.".to_string()))
                        .await?;
                } else {
                    let msg = if let Some(ref sys) = self.session.system_prompt {
                        format!("Current system prompt: {}", sys)
                    } else {
                        "No system prompt set.".to_string()
                    };
                    self.interface.emit(AgentOutput::Status(msg)).await?;
                }
            }
            InterfaceCommand::Debug(subcmd) => {
                // Handle debug commands
                let msg = format!("Debug: {}", subcmd);
                self.interface.emit(AgentOutput::Status(msg)).await?;
            }
        }

        Ok(true)
    }
}
