## ARCHITECTURE.md
A design document detailing Quick‑Query’s architecture: core philosophy, crate structure, message model, provider & tool abstractions, execution flows, configuration layers, tool system, agent system, TUI layout, extension points, and key design decisions (pure Rust UI, bundled SQLite, streaming‑first).

## CHANGELOG.md
Chronological log of project changes. Highlights unreleased work and version **0.1.0** (Feb 2026) introducing one‑shot completion, interactive chat/TUI, profile management, OpenAI‑compatible providers, built‑in tools (filesystem, memory, web, process data), agent framework (chat, explore, researcher, coder, reviewer, summarizer, planner, writer), security defaults, and performance notes.

## README.md
Top‑level overview of Quick‑Query: a fast, extensible CLI for LLMs. Lists core features, quick start commands, usage examples, built‑in agents, CLI options/commands, documentation links, crate overviews, configuration guide, build/test steps, contribution workflow, and troubleshooting tips.

## crates/qq-agents/README.md
Describes the `qq-agents` crate: internal agent definitions (chat, explore, researcher, coder, reviewer, summarizer, planner, writer). Provides a table of agents, usage examples for instantiating agents programmatically, and explains the required `InternalAgent` trait.

## crates/qq-agents/src/chat.rs
Implements **ChatAgent**, the default interactive coordinator. Contains a system prompt forcing delegation to specialized agents only, optional custom prompts, agents‑only mode, and a full `InternalAgent` implementation (no tools, max turns). Includes unit tests.

## crates/qq-agents/src/coder.rs
Defines **CoderAgent** for autonomous code generation/modification. Provides a detailed coding workflow prompt, required tools (`read_file`, `edit_file`, `write_file`, etc.) with usage limits, and implements the `InternalAgent` trait (max turns = 100). Includes unit tests.

## crates/qq-agents/src/config.rs
Configuration structures for agents: `BuiltinAgentOverride` (optional max_turns & per‑tool limits) and `AgentDefinition` (description, system_prompt, provider/model overrides, tool list, max_turns, tool_limits). Loads `agents.toml`, offers lookup helpers, and includes comprehensive tests.

## crates/qq-agents/src/explore.rs
Implements **ExploreAgent**, a read‑only filesystem explorer. System prompt details exploration strategy; tools: `read_file`, `find_files`, `search_files` with limits. Implements `InternalAgent` (max turns = 100) and includes unit tests.

## crates/qq-agents/src/lib.rs
Exports all agent structs and the core **InternalAgent** trait (required methods: name, description, system_prompt, tool_names, optional max_turns, tool_description, tool_limits). Provides `AgentInfo` and `InternalAgentType` enum for listing, creating, and parsing agents. Includes tests.

## crates/qq-agents/src/planner.rs
Defines **PlannerAgent** for task decomposition. System prompt outlines planning responsibilities and output format. Uses `read_memory` tool (limit = 5). Implements `InternalAgent` (max turns = 100) with detailed tool description and unit tests.

## crates/qq-agents/src/researcher.rs
Implements **ResearcherAgent**, an autonomous web researcher. System prompt distinguishes fast vs. in‑depth research, workflow steps, and tool usage (`web_search`, `fetch_webpage`, `read_memory` with limits). Implements `InternalAgent` (max turns = 100) and provides comprehensive tests.

## crates/qq-agents/src/reviewer.rs
Implements **ReviewerAgent**, an autonomous code reviewer. System prompt covers review process, severity categories, and required tools (`read_file`, `find_files`, `search_files`). Implements `InternalAgent` (max turns = 100) with a detailed tool description and tests.

## crates/qq-agents/src/summarizer.rs
Implements **SummarizerAgent**, which creates tailored content summaries. System prompt guides summarization styles (executive, action‑focused, problem‑focused, learning‑focused). No tools needed; max turns = 100. Includes tool description and unit tests.

## crates/qq-agents/src/writer.rs
Implements **WriterAgent**, a documentation generator. System prompt stresses specifying output destinations (file path or return as response) and writing strategies. Tools: `read_file`, `write_file`, `edit_file`, etc., with limits. Implements `InternalAgent` (max turns = 100) and provides tests.

## crates/qq-cli/README.md
Describes the `qq-cli` crate: binary that aggregates all other crates, loads TOML config, creates providers/tools/agents, supports one‑shot, interactive chat/TUI, and legacy readline mode. Covers installation, quick start commands, CLI reference options (prompt, profile, model, system prompt, temperature, etc.) and detailed TUI architecture.

## crates/qq-cli/src/agents/agent_tool.rs
Provides **InternalAgentTool** & **ExternalAgentTool**, wrapper tools exposing agents as callable LLM tools. Handles nesting depth, tool limits, context propagation, agent execution with continuation support, and event‑bus notifications. Includes `create_agent_tools` to generate enabled agent tools.

## crates/qq-cli/src/agents/continuation.rs
Implements continuation logic for agents hitting `max_turns`. Generates a summary via LLM, formats it into context, re‑executes the agent up to configurable continuations. Defines configuration structs, execution result types, and safe message splitting helpers.

## crates/qq-cli/src/agents/inform_user.rs
Defines **InformUserTool**, allowing agents to send non‑blocking status messages to the user via the event bus (`UserNotification` events). Implements argument parsing, tool definition with required `message`, and execution that publishes notifications.

## crates/qq-cli/src/agents/mod.rs
Re‑exports agent tools & continuation types. Also re‑exports internal agents from `qq-agents`. Defines **AgentExecutor** to build tool subsets per agent, run internal/external agents, list enabled agents, check existence, and provides async execution helpers with tests.

## crates/qq-cli/src/chat.rs
Implements interactive chat (readline mode). Manages messages, system prompt, context compaction (LLM summarization or truncation), tool execution, streaming responses, markdown rendering, tool call display, debug logging, command parsing (`/help`, `/quit`, etc.). Provides utilities for byte formatting, RSS measurement, safe message splitting, and continuation support.

## crates/qq-cli/src/config.rs
Defines CLI configuration: loads `config.toml` (profiles, providers, prompts, tools), resolves layered overrides (TOML → env → CLI flags), provides helpers to retrieve active profile settings, provider configurations, and tool toggles. Includes validation and defaults.

## crates/qq-cli/src/debug_log.rs
Implements **DebugLogger**, optional file‑based logger for detailed diagnostics: logs conversation start, iteration info, messages sent/received, tool calls/results, token usage, final assistant output. Enabled via `--debug-file` flag.

## crates/qq-cli/src/event_bus.rs
Provides asynchronous event bus (`AgentEventBus`) using Tokio broadcast channels. Defines events (tool start/complete, progress updates, user notifications, continuation) enabling agents and UI to publish/subscribe in real time.

## crates/qq-cli/src/execution_context.rs
Tracks nested execution stack of agents for proper scoping. Allows pushing/popping agent names, retrieving current scope path, and integrates with per‑agent memory handling.

## crates/qq-cli/src/main.rs
CLI entry point (`clap`). Parses global options (prompt, profile, model, system prompt, temperature, etc.), sets up configuration, provider, tool registry, event bus, optional debug logger, and runs either one‑shot completion or interactive chat. Handles errors gracefully.

## crates/qq-cli/src/markdown.rs
Implements **MarkdownRenderer** using `termimad` to render streamed markdown in the terminal. Accumulates chunks, handles code block syntax highlighting, and provides methods to push content and finalize rendering.

## crates/qq-cli/src/tui/app.rs
Defines main TUI app state (`App`) and run loop with Ratatui. Manages UI components (message list, input area, status bar, agent progress panel), processes events from user input, tool calls, and agent progress via the event bus, and coordinates frame rendering.

## crates/qq-cli/src/tui/events.rs
Enumerates UI events (`UiEvent`) like key presses, mouse clicks, resize, exit signals. Provides conversion helpers from crossterm events to `UiEvent`.

## crates/qq-cli/src/tui/layout.rs
Contains layout calculations for TUI widgets: determines rectangles for message area, input box, status bar, side panel based on terminal size and split ratios.

## crates/qq-cli/src/tui/markdown.rs
Utility for rendering markdown inside TUI widgets using termimad. Provides functions to create a styled `TextArea` and update it dynamically.

## crates/qq-cli/src/tui/mod.rs
Top‑level module exposing the `tui` submodule, re‑exporting UI components and entry point (`run_tui`) for interactive chat.

## crates/qq-cli/src/tui/scroll.rs
Implements vertical scrolling logic for a list of lines (used in message history). Handles scroll offsets, page up/down, and bounds safety.

## crates/qq-cli/src/tui/ui.rs
Builds overall UI layout with Ratatui. Instantiates widget components (`MessageList`, `InputArea`, `StatusBar`, etc.), applies styling, composes them into a frame for each render tick.

## crates/qq-cli/src/tui/widgets/content_area.rs
Widget displaying chat message history with markdown rendering, scrolling, and tool call result highlighting. Provides methods to update content and manage view offsets.

## crates/qq-cli/src/tui/widgets/input_area.rs
Input widget capturing user text (including multi‑line editing), showing a cursor, supporting command shortcuts, and sending entered messages to the chat engine. Handles line wrapping and input history integration.

## crates/qq-cli/src/tui/widgets/mod.rs
Re‑exports all TUI widget modules (`content_area`, `input_area`, `status_bar`, `thinking_panel`) for easier imports elsewhere in UI code.

## crates/qq-cli/src/tui/widgets/status_bar.rs
Displays a status bar at the bottom of the TUI showing current profile, model, token usage, and other runtime metrics. Updates dynamically based on chat state.

## crates/qq-cli/src/tui/widgets/thinking_panel.rs
Shows the “thinking” section during streaming responses: renders partial assistant output (including tool calls) in real time with visual distinction from finalized messages.

## crates/qq-core/README.md
Brief documentation for `qq-core`: core abstractions for messages, providers, tools, agents, and task management. Describes key traits (`Provider`, `Tool`, `Agent`) and their roles.

## crates/qq-core/src/agent.rs
Core **Agent** implementation handling a single agentic loop: runs LLM completion with tool execution, tracks iterations, maintains message history, supports progress handlers, provides both stateless (`run_once`) and stateful interfaces.

## crates/qq-core/src/blocking.rs
Utility offering synchronous wrappers around async core functions (e.g., blocking `run_once`), using Tokio runtime for non‑async contexts.

## crates/qq-core/src/chunker.rs
Implements **ChunkProcessor**, splitting large tool outputs into manageable chunks, optionally summarizing them via LLM, respecting size and chunk count thresholds. Used by agents handling big data.

## crates/qq-core/src/error.rs
Defines a comprehensive `Error` enum for the core library: provider errors, tool execution failures, serialization issues, unknown errors, etc., with proper `std::error::Error` and display implementations.

## crates/qq-core/src/lib.rs
Root module re‑exporting core types (`Agent`, `Provider`, `Tool`, `Message`, `Task`, etc.) and utility functions. Sets up the public API for other crates.

## crates/qq-core/src/message.rs
Defines **Message** struct representing conversation turns, roles (System/User/Assistant/Tool), content handling, tool calls/results, and byte size utilities used for context management.

## crates/qq-core/src/provider.rs
Trait `Provider` abstracts LLM services: synchronous completion (`complete`) and streaming (`stream`), plus capability queries (supports tools, vision). Includes request/response types.

## crates/qq-core/src/task.rs
Implements asynchronous task handling utilities (`TaskHandle`, `TaskManager`) for running background operations like parallel tool execution with cancellation support.

## crates/qq-core/src/tool.rs
Defines the **Tool** trait: name, description, JSON schema definition, async `execute`. Includes `ToolDefinition` and related structs used by `ToolRegistry`.

## crates/qq-providers/README.md
Documentation for `qq-providers`: currently implements an OpenAI provider with streaming support, API key handling, model selection, optional vision capabilities.

## crates/qq-providers/src/lib.rs
Exports provider implementations (currently only OpenAI) and re‑exports core types for external use.

## crates/qq-providers/src/openai.rs
Implements `OpenAIProvider` conforming to `Provider`: builds HTTP requests, handles authentication via API key, streams responses, parses tool calls, supports optional vision (image URLs), with error handling for OpenAI-specific response formats.

## crates/qq-tools/README.md
Overview of built‑in tools in `qq-tools`. Lists filesystem tools (`read_file`, `write_file`, `list_files`, `search_files`), memory persistence tools (SQLite key/value store), web tools (`fetch_webpage`, `web_search`), and large‑data processor (`process_large_data`).

## crates/qq-tools/src/filesystem.rs
Implements filesystem tools: reading files with optional grep/head/tail, writing files, listing directory contents, searching across files using regex. Enforces sandbox root & write permission flags.

## crates/qq-tools/src/lib.rs
Exports all tool implementations (`filesystem`, `memory`, `web`, `process_data`) and provides functions to create default tool sets for the CLI.

## crates/qq-tools/src/memory.rs
Provides memory tools backed by SQLite: `memory_store`, `memory_get`, `memory_list`, `memory_delete`. Supports scoped storage per agent, efficient retrieval, and cleanup of old entries.

## crates/qq-tools/src/process_data.rs
Implements `process_large_data` tool: chunks large text/binary data according to thresholds, optionally runs summarization LLM on each chunk, returns aggregated results. Used by agents for massive outputs.

## crates/qq-tools/src/web.rs
Web tools for fetching webpages (`fetch_webpage`) and performing web searches (`web_search`). Supports optional Perplexica backend configuration, request timeouts, sanitizes HTML before converting to markdown.

## docs/PRD.md
Product Requirements Document for Quick‑Query. Covers vision, architecture (crate breakdown), core component details, feature list (current v0.1.x and roadmap), configuration model, CLI options, streaming architecture, parallel execution patterns, agent framework usage, dependencies, performance goals, security considerations, compatibility notes.

## examples/README.md
Guide to example configuration files in `examples/`. Demonstrates various profile setups (basic, full, local‑LLM, multi‑provider, OpenAI‑compatible) and custom agents definitions (`agents.toml`). Shows how each TOML illustrates different configuration aspects.