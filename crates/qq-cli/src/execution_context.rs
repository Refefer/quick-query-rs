//! Execution context tracking for the TUI.
//!
//! Tracks the current execution stack (Chat > Agent > Tool) for display in the status bar.

use std::sync::Arc;
use tokio::sync::RwLock;

/// Type of execution context entry
#[derive(Debug, Clone, PartialEq)]
pub enum ContextType {
    /// Top-level chat
    Chat,
    /// An agent (internal or external)
    Agent,
    /// A tool being executed
    Tool,
}

/// A single entry in the execution context stack
#[derive(Debug, Clone)]
pub struct ContextEntry {
    pub context_type: ContextType,
    pub name: String,
}

impl ContextEntry {
    pub fn chat() -> Self {
        Self {
            context_type: ContextType::Chat,
            name: "Chat".to_string(),
        }
    }

    pub fn agent(name: impl Into<String>) -> Self {
        Self {
            context_type: ContextType::Agent,
            name: name.into(),
        }
    }

    pub fn tool(name: impl Into<String>) -> Self {
        Self {
            context_type: ContextType::Tool,
            name: name.into(),
        }
    }
}

/// Shared execution context stack
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    stack: Arc<RwLock<Vec<ContextEntry>>>,
}

impl Default for ExecutionContext {
    fn default() -> Self {
        Self::new()
    }
}

impl ExecutionContext {
    pub fn new() -> Self {
        Self {
            stack: Arc::new(RwLock::new(vec![ContextEntry::chat()])),
        }
    }

    /// Push a new context entry onto the stack
    pub async fn push(&self, entry: ContextEntry) {
        let mut stack = self.stack.write().await;
        stack.push(entry);
    }

    /// Pop the top context entry from the stack
    pub async fn pop(&self) {
        let mut stack = self.stack.write().await;
        // Never pop the root Chat entry
        if stack.len() > 1 {
            stack.pop();
        }
    }

    /// Push an agent context
    pub async fn push_agent(&self, name: &str) {
        self.push(ContextEntry::agent(name)).await;
    }

    /// Push a tool context
    pub async fn push_tool(&self, name: &str) {
        self.push(ContextEntry::tool(name)).await;
    }

    /// Clear all entries except the root Chat
    pub async fn reset(&self) {
        let mut stack = self.stack.write().await;
        stack.truncate(1);
    }

    /// Get the current stack for display
    pub async fn get_stack(&self) -> Vec<ContextEntry> {
        self.stack.read().await.clone()
    }

    /// Format the stack for display (e.g., "Chat > Agent[Explore] > Tool[list_files]")
    pub async fn format(&self) -> String {
        let stack = self.stack.read().await;
        stack
            .iter()
            .map(|entry| match entry.context_type {
                ContextType::Chat => "Chat".to_string(),
                ContextType::Agent => format!("Agent[{}]", entry.name),
                ContextType::Tool => format!("Tool[{}]", entry.name),
            })
            .collect::<Vec<_>>()
            .join(" > ")
    }

    /// Get a blocking snapshot of the formatted stack (for sync contexts)
    pub fn format_blocking(&self) -> String {
        // Use try_read for non-blocking access in render loops
        if let Ok(stack) = self.stack.try_read() {
            stack
                .iter()
                .map(|entry| match entry.context_type {
                    ContextType::Chat => "Chat".to_string(),
                    ContextType::Agent => format!("Agent[{}]", entry.name),
                    ContextType::Tool => format!("Tool[{}]", entry.name),
                })
                .collect::<Vec<_>>()
                .join(" > ")
        } else {
            "Chat".to_string()
        }
    }

    /// Check if there's any active execution beyond Chat
    pub fn is_active(&self) -> bool {
        if let Ok(stack) = self.stack.try_read() {
            stack.len() > 1
        } else {
            false
        }
    }

    /// Get the current activity description as full call stack
    /// Returns a human-readable string like "Chat > Tool[list_files]"
    pub fn current_activity_blocking(&self) -> Option<String> {
        if let Ok(stack) = self.stack.try_read() {
            if stack.len() <= 1 {
                return None; // Only Chat, no activity
            }

            // Build full call stack string with type prefixes
            let formatted: Vec<String> = stack
                .iter()
                .map(|entry| match entry.context_type {
                    ContextType::Chat => "Chat".to_string(),
                    ContextType::Agent => format!("Agent[{}]", entry.name),
                    ContextType::Tool => format!("Tool[{}]", entry.name),
                })
                .collect();

            Some(formatted.join(" > "))
        } else {
            None
        }
    }
}

/// RAII guard for context entries - automatically pops when dropped
pub struct ContextGuard {
    context: ExecutionContext,
}

impl ContextGuard {
    pub fn new(context: ExecutionContext) -> Self {
        Self { context }
    }

    /// Pop the context entry (called on drop or explicitly)
    pub async fn pop(self) {
        self.context.pop().await;
    }
}
